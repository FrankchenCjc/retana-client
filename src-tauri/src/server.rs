// Local WebSocket server + wss proxy to remote bridge.
//
// Accepts frontend connections on ws://127.0.0.1:PORT
// Proxies frontend chat messages to remote wss:// bridge (encrypted, no cert verify)
// Forwards bridge responses back to frontend via broadcast.
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

/// Start local WS server + bridge proxy.
pub async fn run_server(port: u16, bridge_url: &str, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("🖥 Local WS server on ws://{}", addr);

    let (tx, _rx) = broadcast::channel::<String>(256);

    // Spawn bridge proxy: connects to remote wss:// bridge
    let proxy_tx = tx.clone();
    let proxy_rx = tx.subscribe();
    let proxy_shutdown = Arc::clone(&shutdown);
    let url = bridge_url.to_string();
    tokio::spawn(async move {
        bridge_proxy(proxy_tx, proxy_rx, &url, proxy_shutdown).await;
    });

    // Accept local frontend clients
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer)) => {
                        log::debug!("New connection from {}", peer);
                        let tx = tx.clone();
                        let rx = tx.subscribe();
                        let shutdown = Arc::clone(&shutdown);
                        tokio::spawn(handle_connection(stream, tx, rx, shutdown));
                    }
                    Err(e) => {
                        log::error!("Accept error: {}", e);
                    }
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

/// Connect to remote wss:// bridge. Forward frontend→bridge and bridge→frontend.
async fn bridge_proxy(
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    url: &str,
    shutdown: Arc<AtomicBool>,
) {
    use tokio_native_tls::native_tls;

    let tls = match native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            log::error!("Bridge proxy: TLS builder failed: {}", e);
            return;
        }
    };
    let connector = tokio_native_tls::TlsConnector::from(tls);

    let (mut ws, _) = match tokio_tungstenite::connect_async_tls_with_config(
        url,
        None,
        false,
        Some(tokio_tungstenite::Connector::NativeTls(connector)),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            log::error!("Bridge proxy: connect to {} failed: {}", url, e);
            return;
        }
    };

    log::info!("🔗 Bridge proxy connected to {}", url);
    tx.send(r#"{"type":"chat","content":"🟢 本地端点已就绪","sender":"system"}"#.into()).ok();

    loop {
        tokio::select! {
            // Frontend → Bridge: only forward user chat messages
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
                            if ws.send(Message::Text(text.into())).await.is_err() {
                                log::error!("Bridge proxy: send failed");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

            // Bridge → Frontend
            ws_msg = ws.next() => {
                match ws_msg {
                    Some(Ok(Message::Text(text))) => {
                        let _ = tx.send(text.to_string());
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        log::warn!("Bridge proxy: disconnected, will retry...");
                        // Broadcast disconnect
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
                        log::debug!("WS recv from {}: {}", peer, &text[..text.len().min(200)]);
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
