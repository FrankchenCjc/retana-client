// Reverse SSH tunnel — forwards connections from the Hermes server
// back to the retana local machine via SSH remote port forwarding.
//
// Flow: Hermes connects to hermes-server:remote_port
//       → SSH tunnels it back → retana localhost:local_port

use anyhow::{Context, Result};
use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Start a persistent reverse SSH tunnel.
///
/// `remote_port` — port on the Hermes server that Hermes will connect to
/// `local_port`  — port on retana (this machine) where our local server listens
///
/// Spawns a background thread that:
/// 1. Opens a reverse-forward listener on the remote (Hermes) side
/// 2. Accepts incoming connections
/// 3. Bidirectionally copies data between the remote channel and local port
pub fn start_reverse_tunnel(
    session: Arc<Session>,
    remote_port: u16,
    local_port: u16,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    log::info!(
        "🌐 Reverse tunnel: Hermes server :{} → retana localhost:{}",
        remote_port,
        local_port
    );

    // Start listening for reverse-forward connections on the remote side.
    // ssh2 0.9 API: channel_forward_listen(port, host, bound_port)
    let mut listener = session
        .channel_forward_listen(remote_port, None, None)
        .context("Failed to start reverse forward listener")?;

    let local_port = local_port;

    thread::Builder::new()
        .name("reverse-tunnel".into())
        .spawn(move || {
            log::info!("Reverse tunnel listener started on remote port {}", remote_port);

            // Set a read timeout so we can check the shutdown flag periodically
            if let Err(e) = session.set_timeout(1000) {
                log::warn!("Failed to set session timeout: {}", e);
            }

            loop {
                if shutdown.load(Ordering::Relaxed) {
                    log::info!("Reverse tunnel shutting down");
                    break;
                }

                match listener.accept() {
                    Ok(mut remote_channel) => {
                        log::info!("Reverse tunnel: accepted connection from remote");

                        // Connect to local service
                        let local_addr = format!("127.0.0.1:{}", local_port);
                        match TcpStream::connect_timeout(
                            &local_addr
                                .parse()
                                .unwrap(),
                            Duration::from_secs(5),
                        ) {
                            Ok(mut local_stream) => {
                                // Bidirectional copy between remote channel and local stream
                                // We need two threads for full duplex, or use non-blocking I/O.
                                // For simplicity, use two threads.

                                let mut remote_read = remote_channel.try_clone()
                                    .expect("Failed to clone remote channel for reading");

                                let read_shutdown = Arc::clone(&shutdown);

                                // Remote → Local
                                let t1 = thread::spawn(move || {
                                    let mut buf = [0u8; 8192];
                                    loop {
                                        if read_shutdown.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        match remote_read.read(&mut buf) {
                                            Ok(0) => break, // EOF
                                            Ok(n) => {
                                                if local_stream.write_all(&buf[..n]).is_err() {
                                                    break;
                                                }
                                                let _ = local_stream.flush();
                                            }
                                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                                thread::sleep(Duration::from_millis(50));
                                                continue;
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                    log::debug!("Reverse tunnel: remote→local done");
                                });

                                // Local → Remote
                                let mut local_read = local_stream
                                    .try_clone()
                                    .expect("Failed to clone local stream");
                                let mut remote_write = remote_channel;

                                let write_shutdown = Arc::clone(&shutdown);

                                let t2 = thread::spawn(move || {
                                    let mut buf = [0u8; 8192];
                                    // Set read timeout on local stream
                                    let _ = local_read.set_read_timeout(Some(Duration::from_millis(500)));
                                    loop {
                                        if write_shutdown.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        match local_read.read(&mut buf) {
                                            Ok(0) => break,
                                            Ok(n) => {
                                                if remote_write.write_all(&buf[..n]).is_err() {
                                                    break;
                                                }
                                            }
                                            Err(ref e)
                                                if e.kind() == std::io::ErrorKind::WouldBlock
                                                    || e.kind() == std::io::ErrorKind::TimedOut =>
                                            {
                                                continue;
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                    log::debug!("Reverse tunnel: local→remote done");
                                });

                                let _ = t1.join();
                                let _ = t2.join();
                            }
                            Err(e) => {
                                log::error!(
                                    "Reverse tunnel: failed to connect to localhost:{} — {}",
                                    local_port,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        if shutdown.load(Ordering::Relaxed) {
                            break;
                        }
                        log::error!("Reverse tunnel accept error: {}", e);
                        // Brief pause before retry
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }

            log::info!("Reverse tunnel thread exited");
        })?;

    Ok(())
}
