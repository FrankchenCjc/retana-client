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
/// `local_port`  — port on retana where our WebSocket server listens
///
/// Spawns a background thread that accepts reverse-forwarded connections
/// and bidirectionally copies data between the remote channel and local port.
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
    // ssh2 0.9: channel_forward_listen returns (Listener, u16)
    let mut listener = session
        .channel_forward_listen(remote_port, None, None)
        .context("Failed to start reverse forward listener")?
        .0; // Extract Listener from tuple

    thread::Builder::new()
        .name("reverse-tunnel".into())
        .spawn(move || {
            log::info!(
                "Reverse tunnel listener started on remote port {}",
                remote_port
            );

            loop {
                if shutdown.load(Ordering::Relaxed) {
                    log::info!("Reverse tunnel shutting down");
                    break;
                }

                match listener.accept() {
                    Ok(mut remote_channel) => {
                        log::info!("Reverse tunnel: accepted connection from remote");

                        let local_addr = format!("127.0.0.1:{}", local_port);
                        match TcpStream::connect_timeout(
                            &local_addr.parse().unwrap(),
                            Duration::from_secs(5),
                        ) {
                            Ok(local_stream) => {
                                // Set session to non-blocking for the tunnel
                                session.set_blocking(false);

                                let _ = local_stream.set_nonblocking(true);

                                // Set read timeouts
                                let _ = local_stream.set_read_timeout(Some(
                                    Duration::from_millis(500),
                                ));

                                let mut buf = [0u8; 16384];
                                let mut local_buf = [0u8; 16384];

                                loop {
                                    if shutdown.load(Ordering::Relaxed) {
                                        break;
                                    }

                                    let mut did_work = false;

                                    // Remote → Local
                                    match remote_channel.read(&mut buf) {
                                        Ok(0) => break, // EOF
                                        Ok(n) => {
                                            if local_stream
                                                .write_all(&buf[..n])
                                                .is_err()
                                            {
                                                break;
                                            }
                                            let _ = local_stream.flush();
                                            did_work = true;
                                        }
                                        Err(ref e)
                                            if e.kind()
                                                == std::io::ErrorKind::WouldBlock =>
                                        {
                                            // No data yet
                                        }
                                        Err(_) => break,
                                    }

                                    // Local → Remote
                                    match local_stream.read(&mut local_buf) {
                                        Ok(0) => break,
                                        Ok(n) => {
                                            if remote_channel
                                                .write_all(&local_buf[..n])
                                                .is_err()
                                            {
                                                break;
                                            }
                                            did_work = true;
                                        }
                                        Err(ref e)
                                            if e.kind()
                                                == std::io::ErrorKind::WouldBlock
                                                || e.kind()
                                                    == std::io::ErrorKind::TimedOut =>
                                        {
                                            // No data yet
                                        }
                                        Err(_) => break,
                                    }

                                    if !did_work {
                                        // Avoid busy-waiting
                                        thread::sleep(Duration::from_millis(10));
                                    }
                                }

                                log::debug!("Reverse tunnel: connection closed");
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
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }

            log::info!("Reverse tunnel thread exited");
        })?;

    Ok(())
}
