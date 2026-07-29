// Local WebSocket server + encrypted proxy to remote bridge.
//
// Frontend connects to ws://127.0.0.1:PORT (local, plaintext)
// Rust proxy connects to plain ws:// bridge, reads its public key from
// the first message, then encrypts all traffic with NaCl sealed box.
//
// Key exchange: bridge sends {"type":"key","pubkey":"hex"} → retana seals.
// Daily key rotation: cron regenerates bridge key, retana picks up on reconnect.
use futures_util::{SinkExt, StreamExt};
use sodiumoxide::crypto::box_;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

/// Start local WS server + encrypted bridge proxy.
pub async fn run_server(port: u16, bridge_url: &str, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    sodiumoxide::init().map_err(|_| anyhow::anyhow!("sodiumoxide init failed"))?;

    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("🖥 Local WS server on ws://{}", addr);

    let (tx, _rx) = broadcast::channel::<String>(256);

    let proxy_tx = tx.clone();
    let proxy_rx = tx.subscribe();
    let proxy_shutdown = Arc::clone(&shutdown);
    let url = bridge_url.to_string();
    tokio::spawn(async move {
        encrypted_bridge_proxy(proxy_tx, proxy_rx, &url, proxy_shutdown).await;
    });

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer)) => {
                        let tx = tx.clone();
                        let rx = tx.subscribe();
                        let shutdown = Arc::clone(&shutdown);
                        tokio::spawn(handle_connection(stream, tx, rx, shutdown));
                    }
                    Err(e) => log::error!("Accept error: {}", e),
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if shutdown.load(Ordering::Relaxed) {
                    log::info!("WebSocket server shutting down");
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Connect to bridge, read its public key announcement, then encrypt all payloads.
async fn encrypted_bridge_proxy(
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    url: &str,
    shutdown: Arc<AtomicBool>,
) {
    let (mut ws, _) = match tokio_tungstenite::connect_async(url).await {
        Ok(c) => c,
        Err(e) => {
            log::error!("Bridge proxy: connect to {} failed: {}", url, e);
            return;
        }
    };

    // Read first message: bridge announces its public key
    let bridge_pk = match ws.next().await {
        Some(Ok(Message::Text(text))) => {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) if v.get("type").and_then(|t| t.as_str()) == Some("key") => {
                    v.get("pubkey")
                        .and_then(|p| p.as_str())
                        .and_then(|h| hex::decode(h).ok())
                        .and_then(|b| box_::PublicKey::from_slice(&b))
                }
                _ => None,
            }
        }
        _ => None,
    };

    let bridge_pk = match bridge_pk {
        Some(pk) => pk,
        None => {
            log::error!("Bridge proxy: failed to receive key announcement");
            return;
        }
    };

    log::info!(
        "🔑 Bridge key: {}...",
        hex::encode(&bridge_pk.as_ref()[..4])
    );

    // Generate ephemeral keypair for this session
    let (epk, esk) = box_::gen_keypair();
    let precomputed = box_::precompute(&bridge_pk, &esk);

    log::info!("🔗 Bridge proxy ready (NaCl encrypted)");
    tx.send(r#"{"type":"chat","content":"🟢 本地端点已就绪","sender":"system"}"#.into()).ok();

    loop {
        tokio::select! {
            // Frontend → Bridge: sealed box
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        let forward = serde_json::from_str::<serde_json::Value>(&text)
                            .map(|v| {
                                v.get("sender").and_then(|s| s.as_str()) == Some("user")
                                    && v.get("type").and_then(|t| t.as_str()) == Some("chat")
                            })
                            .unwrap_or(false);
                        if forward {
                            let mut payload = Vec::from(epk.as_ref());
                            payload.extend_from_slice(text.as_bytes());
                            let sealed = box_::seal(&payload, &bridge_pk);
                            if ws.send(Message::Binary(sealed.into())).await.is_err() {
                                log::error!("Bridge proxy: send failed");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

            // Bridge → Frontend: box decrypt
            ws_msg = ws.next() => {
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        if data.len() < 24 { continue; }
                        let nonce = match box_::Nonce::from_slice(&data[..24]) {
                            Some(n) => n,
                            None => continue,
                        };
                        match box_::open(&data[24..], &nonce, &precomputed) {
                            Ok(plain) => {
                                if let Ok(text) = String::from_utf8(plain) {
                                    let _ = tx.send(text);
                                }
                            }
                            Err(_) => log::error!("Bridge proxy: decrypt failed"),
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        let _ = tx.send(text.to_string());
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        log::warn!("Bridge proxy: disconnected");
                        tx.send(r#"{"type":"chat","content":"🔴 连接已断开","sender":"system"}"#.into()).ok();
                        break;
                    }
                    Some(Err(e)) => {
                        log::error!("Bridge proxy: read error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if shutdown.load(Ordering::Relaxed) { break; }
            }
        }
    }

    let _ = ws.close().await;
}

async fn handle_connection(
    stream: TcpStream,
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    shutdown: Arc<AtomicBool>,
) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_else(|_| "unknown".into());
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log::error!("WebSocket handshake failed for {}: {}", peer, e);
            return;
        }
    };
    log::info!("WebSocket connected: {}", peer);
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    loop {
        tokio::select! {
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => { let _ = tx.send(text.to_string()); }
                    Some(Ok(Message::Ping(data))) => { let _ = ws_sender.send(Message::Pong(data)).await; }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            broadcast_msg = rx.recv() => {
                match broadcast_msg {
                    Ok(msg) => {
                        if ws_sender.send(Message::Text(msg.into())).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("Client {} lagged by {} messages", peer, n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if shutdown.load(Ordering::Relaxed) { break; }
            }
        }
    }
    let _ = ws_sender.close().await;
}
