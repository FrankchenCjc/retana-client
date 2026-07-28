// Local WebSocket server — the endpoint that both retana frontend and Hermes
// (through the reverse SSH tunnel) connect to for real-time chat.
//
// Runs on 127.0.0.1:9000 (configurable).
// Simple broadcast model: every message from any client is relayed to all others.

use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

/// Start the local WebSocket server.
/// Returns when the server exits (only if `shutdown` is triggered).
pub async fn run_server(port: u16, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("🖥  Local server listening on ws://{}", addr);

    // Broadcast channel: all connected clients receive messages
    let (tx, _rx) = broadcast::channel::<String>(256);

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
            // Messages from this client → broadcast to all others
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        log::debug!("WS recv from {}: {}", peer, &text[..text.len().min(200)]);
                        let _ = tx.send(text);
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = ws_sender.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        log::info!("WebSocket disconnected: {}", peer);
                        break;
                    }
                    Some(Ok(_)) => {} // Ignore binary
                    Some(Err(e)) => {
                        log::error!("WebSocket error from {}: {}", peer, e);
                        break;
                    }
                }
            }
            // Broadcast messages from other clients → this client
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
