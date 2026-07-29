// Local WebSocket server + encrypted proxy to remote bridge.
//
// Frontend connects to ws://127.0.0.1:PORT (local, plaintext)
// Rust proxy encrypts with NaCl sealed box → plain ws:// bridge
// Bridge decrypts, processes, encrypts response with NaCl box → Rust proxy decrypts
//
// Key exchange: retana ephemeral keypair, bridge's NaCl public key.
use futures_util::{SinkExt, StreamExt};
use sodiumoxide::crypto::box_;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

/// Bridge's NaCl public key (32 bytes hex).
const BRIDGE_PK_HEX: &str = "f755e6b5773487948b79940c5ea44ad1f174885117cfb725a660eb7a9186065d";

/// Start local WS server + encrypted bridge proxy.
pub async fn run_server(port: u16, bridge_url: &str, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    // Init sodiumoxide
    sodiumoxide::init().map_err(|_| anyhow::anyhow!("sodiumoxide init failed"))?;

    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("🖥 Local WS server on ws://{}", addr);

    let (tx, _rx) = broadcast::channel::<String>(256);

    // Spawn encrypted bridge proxy
    let proxy_tx = tx.clone();
    let proxy_rx = tx.subscribe();
    let proxy_shutdown = Arc::clone(&shutdown);
    let url = bridge_url.to_string();
    tokio::spawn(async move {
        encrypted_bridge_proxy(proxy_tx, proxy_rx, &url, proxy_shutdown).await;
    });

    // Accept local frontend clients
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

/// Connect to plain ws:// bridge. Encrypt payloads with NaCl sealed box / box.
async fn encrypted_bridge_proxy(
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    url: &str,
    shutdown: Arc<AtomicBool>,
) {
    // Generate ephemeral keypair for this session
    let (epk, esk) = box_::gen_keypair();
    let bridge_pk = match hex::decode(BRIDGE_PK_HEX).ok()
        .and_then(|b| box_::PublicKey::from_slice(&b))
    {
        Some(pk) => pk,
        None => {
            log::error!("Bridge proxy: invalid public key");
            return;
        }
    };

    // Connect to bridge (plain ws://)
    let (mut ws, _) = match tokio_tungstenite::connect_async(url).await {
        Ok(c) => c,
        Err(e) => {
            log::error!("Bridge proxy: connect to {} failed: {}", url, e);
            return;
        }
    };

    log::info!("🔗 Bridge proxy connected to {} (NaCl encrypted)", url);
    tx.send(r#"{"type":"chat","content":"🟢 本地端点已就绪","sender":"system"}"#.into()).ok();

    let precomputed = box_::precompute(&bridge_pk, &esk);

    loop {
        tokio::select! {
            // Frontend → Bridge: encrypt with sealed box
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
                            // SealedBox(epk || message) → bridge decrypts, extracts epk
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

            // Bridge → Frontend: decrypt with box (nonce prepended by pynacl)
            ws_msg = ws.next() => {
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        // Split nonce(24) | ciphertext
                        if data.len() < 24 {
                            log::error!("Bridge proxy: response too short");
                            continue;
                        }
                        let nonce = match box_::Nonce::from_slice(&data[..24]) {
                            Some(n) => n,
                            None => { log::error!("Bridge proxy: bad nonce"); continue; }
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
                        // Fallback: unencrypted response (e.g. system messages)
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
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    }

    let _ = ws.close().await;
    log::info!("Bridge proxy closed");
}

async fn handle_connection(
    stream: TcpStream,
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    shutdown: Arc<AtomicBool>,
) {
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".into());

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
                    Some(Ok(Message::Text(text))) => {
                        let _ = tx.send(text.to_string());
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = ws_sender.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        log::info!("WebSocket disconnected: {}", peer);
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        log::error!("WebSocket error from {}: {}", peer, e);
                        break;
                    }
                }
            }

            broadcast_msg = rx.recv() => {
                match broadcast_msg {
                    Ok(msg) => {
                        if ws_sender.send(Message::Text(msg.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("Client {} lagged by {} messages", peer, n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
    }

    let _ = ws_sender.close().await;
}
