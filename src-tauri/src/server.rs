// Local WebSocket server + encrypted proxy to remote bridge.
//
// Frontend connects to ws://127.0.0.1:PORT (local, plaintext)
// Rust proxy connects to ws:// bridge, receives public key on connect,
// encrypts all traffic with NaCl sealed box / box.
//
// Key rotation: bridge sends {"type":"key_rot","pubkey":"hex"} encrypted
// with OLD key. Retana decrypts, switches to new key for subsequent messages.
use futures_util::{SinkExt, StreamExt};
use sodiumoxide::crypto::{box_, sealedbox};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

/// Execute a local command and return structured result
fn exec_local(cmd: &str) -> serde_json::Value {
    let output = if cfg!(target_os = "windows") {
        // cmd /U = UTF-16LE output for internal commands (dir, type, etc.)
        // chcp 65001 = UTF-8 for external commands — belt and suspenders
        let effective = format!("chcp 65001 > nul & {}", cmd);
        Command::new("cmd").args(["/U", "/C", &effective]).output()
    } else {
        Command::new("sh").args(["-c", cmd]).output()
    };
    match output {
        Ok(o) => {
            // cmd /U produces UTF-16LE — decode accordingly
            let decoded = if cfg!(target_os = "windows") {
                decode_cmd_output(&o.stdout, &o.stderr)
            } else {
                (String::from_utf8_lossy(&o.stdout).to_string(),
                 String::from_utf8_lossy(&o.stderr).to_string())
            };
            let combined = if decoded.0.is_empty() { decoded.1 } else { decoded.0 };
            // Clean control chars (except newline/tab) that can break JSON
            let cleaned: String = combined.chars().map(|c| {
                if c.is_control() && c != '\n' && c != '\t' { ' ' } else { c }
            }).collect();
            serde_json::json!({
                "output": cleaned,
                "exit_code": o.status.code().unwrap_or(-1),
                "success": o.status.success()
            })
        }
        Err(e) => serde_json::json!({
            "output": e.to_string(),
            "exit_code": -1,
            "success": false
        }),
    }
}

/// Decode cmd /U (UTF-16LE) output, with UTF-8 fallback
fn decode_cmd_output(stdout: &[u8], stderr: &[u8]) -> (String, String) {
    (decode_best_effort(stdout), decode_best_effort(stderr))
}

fn decode_best_effort(bytes: &[u8]) -> String {
    // Try UTF-16LE first (cmd /U output)
    if bytes.len() >= 2 {
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&u16s);
        // If it looks reasonable (not full of replacement chars), keep it
        let replacement_ratio = s.chars().filter(|&c| c == '\u{FFFD}').count() as f32
            / s.chars().count().max(1) as f32;
        if replacement_ratio < 0.5 {
            return s.trim_end_matches('\0').to_string();
        }
    }
    // Fallback: try UTF-8
    String::from_utf8_lossy(bytes).to_string()
}

/// Smart truncation: head 2 lines + "... N lines omitted ..." + tail 2 lines
fn truncate_detail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= 4 {
        return output.to_string();
    }
    let head = &lines[..2];
    let tail = &lines[lines.len() - 2..];
    let omitted = lines.len() - 4;
    format!(
        "{}\n  … {} lines omitted …\n{}",
        head.join("\n"),
        omitted,
        tail.join("\n")
    )
}

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
                    Ok((stream, _peer)) => {
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
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Mutable bridge state: just the public key (rotates on key_rot)
struct BridgeState {
    pk: box_::PublicKey,
}

impl BridgeState {
    fn rotate(&mut self, new_pk: box_::PublicKey) {
        self.pk = new_pk;
        log::info!("🔑 Key rotated → {}...", hex::encode(&self.pk.as_ref()[..4]));
    }
}

async fn encrypted_bridge_proxy(
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    url: &str,
    shutdown: Arc<AtomicBool>,
) {
    let (mut ws, _) = match tokio_tungstenite::connect_async(url).await {
        Ok(c) => c,
        Err(e) => {
            log::error!("Bridge proxy: connect failed: {}", e);
            return;
        }
    };

    // Bootstrap: read bridge's public key (plaintext first message)
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
            log::error!("Bridge proxy: no key announcement");
            return;
        }
    };

    let (epk, esk) = box_::gen_keypair();
    let state = Arc::new(Mutex::new(BridgeState { pk: bridge_pk }));

    log::info!("🔗 Bridge proxy ready (NaCl, mutable key)");
    tx.send(r#"{"type":"chat","content":"🟢 本地端点已就绪","sender":"system"}"#.into()).ok();

    loop {
        tokio::select! {
            // Frontend → Bridge: SealedBox (one-way, no nonce needed)
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        let forward = serde_json::from_str::<serde_json::Value>(&text)
                            .map(|v| {
                                let t = v.get("type").and_then(|t| t.as_str());
                                let s = v.get("sender").and_then(|s| s.as_str());
                                t == Some("chat") && s == Some("user")
                                    || t == Some("env_info")   // forward env info too
                                    || t == Some("tool_result")  // forward command execution results
                            })
                            .unwrap_or(false);
                        if forward {
                            let pk = { state.lock().unwrap().pk.clone() };
                            let mut payload = Vec::from(epk.as_ref());
                            payload.extend_from_slice(text.as_bytes());
                            let sealed = sealedbox::seal(&payload, &pk);
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

            // Bridge → Frontend: Box decrypt (nonce(24) || ciphertext)
            ws_msg = ws.next() => {
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        if data.len() < 24 { continue; }
                        let nonce = match box_::Nonce::from_slice(&data[..24]) {
                            Some(n) => n,
                            None => continue,
                        };
                        let pk = { state.lock().unwrap().pk.clone() };
                        match box_::open(&data[24..], &nonce, &pk, &esk) {
                            Ok(plain) => {
                                if let Ok(text) = String::from_utf8(plain.clone()) {
                                    // Check for key rotation
                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                        let msg_type = v.get("type").and_then(|t| t.as_str());

                                        if msg_type == Some("key_rot") {
                                            if let Some(h) = v.get("pubkey").and_then(|p| p.as_str()) {
                                                if let Ok(b) = hex::decode(h) {
                                                    if let Some(new_pk) = box_::PublicKey::from_slice(&b) {
                                                        state.lock().unwrap().rotate(new_pk);
                                                        continue;
                                                    }
                                                }
                                            }
                                        }

                                        // Intercept tool_call: execute locally, send result back to bridge
                                        if msg_type == Some("tool_call") {
                                            let task_id = v.get("task_id").and_then(|t| t.as_str()).unwrap_or("").to_string();
                                            let command = v.get("command").and_then(|c| c.as_str()).unwrap_or("").to_string();
                                            let label = v.get("label").and_then(|l| l.as_str()).unwrap_or(&command).to_string();

                                            log::info!("🔧 EXEC: {}", &command);

                                            // Notify frontend: execution started
                                            let start_msg = serde_json::json!({
                                                "type": "tool_progress",
                                                "label": format!("📋 {}", label),
                                                "tool_type": "tool_call",
                                                "status": "running"
                                            });
                                            let _ = tx.send(start_msg.to_string());

                                            // Execute locally in thread pool (non-blocking for tokio)
                                            let cmd = command.clone();
                                            let result = match tokio::task::spawn_blocking(move || exec_local(&cmd)).await {
                                                Ok(r) => r,
                                                Err(e) => serde_json::json!({
                                                    "output": format!("exec error: {}", e),
                                                    "exit_code": -1,
                                                    "success": false
                                                }),
                                            };

                                            // Send result back to bridge (encrypted)
                                            let result_msg = serde_json::json!({
                                                "type": "tool_result",
                                                "task_id": task_id,
                                                "output": result["output"],
                                                "exit_code": result["exit_code"],
                                                "success": result["success"]
                                            });
                                            let pk = { state.lock().unwrap().pk.clone() };
                                            let mut payload = Vec::from(epk.as_ref());
                                            payload.extend_from_slice(result_msg.to_string().as_bytes());
                                            let sealed = sealedbox::seal(&payload, &pk);
                                            if ws.send(Message::Binary(sealed.into())).await.is_err() {
                                                log::error!("Bridge proxy: tool_result send failed");
                                                break;
                                            }

                                            // Notify frontend: done
                                            let done_status = if result["success"].as_bool().unwrap_or(false) { "done" } else { "error" };
                                            let detail = truncate_detail(result["output"].as_str().unwrap_or(""));
                                            log::info!("🔧 EXEC done [{}]: {}", done_status, &detail.lines().next().unwrap_or(""));
                                            let done_msg = serde_json::json!({
                                                "type": "tool_progress",
                                                "label": format!("📋 {}", label),
                                                "tool_type": "tool_call",
                                                "status": done_status,
                                                "detail": detail
                                            });
                                            let _ = tx.send(done_msg.to_string());
                                            continue;
                                        }
                                    }
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
                        tx.send(r#"{"type":"chat","content":"🔴 连接已断开","sender":"system"}"#.into()).ok();
                        break;
                    }
                    Some(Err(_)) => break,
                    _ => {}
                }
            }

            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if shutdown.load(Ordering::Relaxed) { break; }
            }
        }
    }
    let _ = ws.close(None).await;
}

async fn handle_connection(
    stream: TcpStream,
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    shutdown: Arc<AtomicBool>,
) {
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => { log::error!("WS handshake fail: {}", e); return; }
    };
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
                    Ok(msg) => { if ws_sender.send(Message::Text(msg.into())).await.is_err() { break; } }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
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
