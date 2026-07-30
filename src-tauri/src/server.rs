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
        let effective = format!("chcp 65001 > nul & {}", cmd);
        Command::new("cmd").args(["/U", "/C", &effective]).output()
    } else {
        Command::new("sh").args(["-c", cmd]).output()
    };
    match output {
        Ok(o) => {
            let decoded = if cfg!(target_os = "windows") {
                decode_cmd_output(&o.stdout, &o.stderr)
            } else {
                (String::from_utf8_lossy(&o.stdout).to_string(),
                 String::from_utf8_lossy(&o.stderr).to_string())
            };
            let combined = if decoded.0.is_empty() { decoded.1 } else { decoded.0 };
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
            "output": e.to_string(), "exit_code": -1, "success": false
        }),
    }
}

fn decode_cmd_output(stdout: &[u8], stderr: &[u8]) -> (String, String) {
    (decode_best_effort(stdout), decode_best_effort(stderr))
}

fn decode_best_effort(bytes: &[u8]) -> String {
    if bytes.len() >= 2 {
        let u16s: Vec<u16> = bytes.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let s = String::from_utf16_lossy(&u16s);
        let ratio = s.chars().filter(|&c| c == '\u{FFFD}').count() as f32
            / s.chars().count().max(1) as f32;
        if ratio < 0.5 { return s.trim_end_matches('\0').to_string(); }
    }
    String::from_utf8_lossy(bytes).to_string()
}



/// Dead-simple: cut every \\c...\\e block mechanically. No validation, no parsing.
fn strip_cmd_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut remaining = text;
    while let Some(pos) = remaining.find("\\c") {
        out.push_str(&remaining[..pos]);
        let after = &remaining[pos + 2..];
        if let Some(e_pos) = after.find("\\e") {
            remaining = &after[e_pos + 2..];
        } else {
            out.push_str("\\c");
            remaining = after;
        }
    }
    out.push_str(remaining);
    out.trim().to_string()
}


pub async fn run_server(port: u16, bridge_url: &str, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    sodiumoxide::init().map_err(|_| anyhow::anyhow!("sodiumoxide init failed"))?;
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    log::info!("🖥 Local WS server on ws://{}", addr);
    let (tx, _rx) = broadcast::channel::<String>(256);
    let url = bridge_url.to_string();
    tokio::spawn(encrypted_bridge_proxy(tx.clone(), tx.subscribe(), url, Arc::clone(&shutdown)));
    loop {
        tokio::select! {
            result = listener.accept() => {
                if let Ok((stream, _)) = result {
                    tokio::spawn(handle_connection(stream, tx.clone(), tx.subscribe(), Arc::clone(&shutdown)));
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if shutdown.load(Ordering::Relaxed) { break; }
            }
        }
    }
    Ok(())
}

// ── Bridge proxy ────────────────────────────────────────

struct BridgeState { pk: box_::PublicKey }
impl BridgeState {
    fn rotate(&mut self, new_pk: box_::PublicKey) {
        self.pk = new_pk;
        log::info!("🔑 Key rotated → {}...", hex::encode(&self.pk.as_ref()[..4]));
    }
}

/// Nonce(24) || ciphertext → plaintext. One call replaces 4 match levels.
fn try_decrypt(data: &[u8], state: &Arc<Mutex<BridgeState>>, esk: &box_::SecretKey) -> Option<String> {
    let nonce = box_::Nonce::from_slice(&data.get(..24)?)?;
    let pk = state.lock().unwrap().pk;
    let plain = box_::open(data.get(24..)?, &nonce, &pk, esk).ok()?;
    String::from_utf8(plain).ok()
}

/// SealedBox helper: epk || payload, then seal with bridge_pk
fn seal_payload(payload: &str, epk: &box_::PublicKey, state: &Arc<Mutex<BridgeState>>) -> Vec<u8> {
    let pk = state.lock().unwrap().pk;
    let mut buf = Vec::from(epk.as_ref());
    buf.extend_from_slice(payload.as_bytes());
    sealedbox::seal(&buf, &pk)
}

/// Notify frontend of tool execution progress — sends per-line output (no truncation, no JSON)
fn notify_progress(tx: &broadcast::Sender<String>, label: &str, status: &str, detail: Option<&str>) {
    let tag = match status {
        "running" => "\\c> ",
        "done"    => "\\c✓ ",
        _         => "\\c✗ ",
    };
    let _ = tx.send(format!("{tag}{label}"));
    if let Some(d) = detail {
        // Per-line \\c= prefix so every line renders as output
        for line in d.lines() {
            let _ = tx.send(format!("\\c= {line}"));
        }
    }
}

/// Execute a single command, notifying frontend of start/end (raw output, no truncation)
async fn exec_one(cmd_str: &str, tx: &broadcast::Sender<String>) -> serde_json::Value {
    let label = if cmd_str.len() > 60 { &cmd_str[..60] } else { cmd_str };
    notify_progress(tx, label, "running", None);
    let cmd = cmd_str.to_string();
    let result = match tokio::task::spawn_blocking(move || exec_local(&cmd)).await {
        Ok(r) => r, Err(e) => serde_json::json!({"output":format!("exec error: {}", e),"exit_code":-1,"success":false}),
    };
    let status = if result["success"].as_bool().unwrap_or(false) { "done" } else { "error" };
    notify_progress(tx, label, status, Some(result["output"].as_str().unwrap_or("")));
    result
}

async fn encrypted_bridge_proxy(
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    url: String,
    shutdown: Arc<AtomicBool>,
) {
    let (mut ws, _) = match tokio_tungstenite::connect_async(&url).await {
        Ok(c) => c, Err(e) => { log::error!("Bridge proxy: connect failed: {}", e); return; }
    };

    // Bootstrap: plaintext key announcement
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
        None => { log::error!("Bridge proxy: no key announcement"); return; }
    };

    let (epk, esk) = box_::gen_keypair();
    let state = Arc::new(Mutex::new(BridgeState { pk: bridge_pk }));
    log::info!("🔗 Bridge proxy ready (NaCl, mutable key)");
    tx.send("\\cs 🟢 本地端点已就绪".into()).ok();

    loop {
        tokio::select! {
            // ── Frontend → Bridge: raw text → sealedbox ──
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        let sealed = seal_payload(&text, &epk, &state);
                        if ws.send(Message::Binary(sealed.into())).await.is_err() {
                            log::error!("Bridge proxy: send failed"); break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }

            // ── Bridge → Frontend: decrypt → string-match dispatch ──
            ws_msg = ws.next() => {
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        let text = match try_decrypt(&data, &state, &esk) {
                            Some(t) => t, None => continue,
                        };

                        // Internal: key rotation (\k <hex_pubkey>)
                        if text.starts_with("\\k ") {
                            let hex_key = &text[3..].trim();
                            if let Some(pk) = hex::decode(hex_key).ok()
                                .and_then(|b| box_::PublicKey::from_slice(&b))
                            { state.lock().unwrap().rotate(pk); }
                            continue;
                        }

                        // Internal intercepts — mechanically find \\c...\\e blocks, execute what's inside
                        // Format: \\c<first 6 chars = id><space><cmd>\\e  (no validation, dead simple)
                        let mut exec_commands: Vec<(String, String)> = Vec::new(); // (id, cmd)
                        let mut remaining = text.as_str();

                        while let Some(c_pos) = remaining.find("\\c") {
                            let after_c = &remaining[c_pos + 2..];
                            if let Some(e_pos) = after_c.find("\\e") {
                                let block = &after_c[..e_pos];
                                // Mechanical split: first 6 chars = id, skip space, rest = cmd
                                let id: String = block.chars().take(6).collect();
                                let cmd = if block.len() > 7 && block.as_bytes()[6] == b' ' {
                                    block[7..].trim().to_string()
                                } else {
                                    block[6..].trim().to_string()
                                };
                                if !cmd.is_empty() {
                                    exec_commands.push((id, cmd));
                                }
                                remaining = &after_c[e_pos + 2..];
                            } else {
                                remaining = after_c;
                            }
                        }

                        if !exec_commands.is_empty() {
                            // Strip \\c<id> <cmd> \\e blocks, forward clean chat to frontend
                            let clean = strip_cmd_blocks(&text);
                            if !clean.is_empty() { let _ = tx.send(clean); }

                            // Multiple commands → list header first
                            if exec_commands.len() > 1 {
                                let _ = tx.send(format!("\\c> ⚡ 批量执行 ({} 条命令):", exec_commands.len()));
                                for (i, (_, cmd)) in exec_commands.iter().enumerate() {
                                    let short = if cmd.len() > 50 { &cmd[..50] } else { cmd.as_str() };
                                    let _ = tx.send(format!("\\c=   [{}/{}] {}", i + 1, exec_commands.len(), short));
                                }
                            }

                            let mut results = Vec::new();
                            for (i, (batch_id, cmd)) in exec_commands.iter().enumerate() {
                                let label = if exec_commands.len() > 1 {
                                    format!("[{}/{}] {}", i + 1, exec_commands.len(), cmd)
                                } else {
                                    cmd.clone()
                                };
                                let r = exec_one(&label, &tx).await;
                                results.push(serde_json::json!({
                                    "batch_id": batch_id, "cmd": cmd,
                                    "output": r["output"], "exit_code": r["exit_code"], "success": r["success"]
                                }));
                            }

                            // Send all results back to bridge: \c<batch_id> <json> \e per result
                            for (batch_id, _) in &exec_commands {
                                let result_json = results.iter()
                                    .find(|r| r["batch_id"].as_str() == Some(batch_id.as_str()))
                                    .cloned().unwrap_or(serde_json::json!({"error":"not found"}));
                                let result_text = format!("\\c{} {} \\e", batch_id, result_json);
                                let sealed = seal_payload(&result_text, &epk, &state);
                                if ws.send(Message::Binary(sealed.into())).await.is_err() { break; }
                            }
                            continue;
                        }

                        // Fallback: forward raw decrypted text to frontend
                        let _ = tx.send(text);
                    }
                    Some(Ok(Message::Text(text))) => { let _ = tx.send(text.to_string()); }
                    Some(Ok(Message::Ping(data))) => { let _ = ws.send(Message::Pong(data)).await; }
                    Some(Ok(Message::Close(_))) | None => {
                        tx.send("\\e 🔴 连接已断开".into()).ok();
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

// ── Local WS connection handler ──────────────────────────

async fn handle_connection(
    stream: TcpStream,
    tx: broadcast::Sender<String>,
    mut rx: broadcast::Receiver<String>,
    shutdown: Arc<AtomicBool>,
) {
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws, Err(e) => { log::error!("WS handshake fail: {}", e); return; }
    };
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();
    loop {
        tokio::select! {
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => { let _ = tx.send(text.to_string()); }
                    Some(Ok(Message::Ping(data))) => { let _ = ws_sender.send(Message::Pong(data)).await; }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
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
