// Reverse SSH tunnel implementation
// Forwards a remote port to the local machine

use crate::ssh::manager::SshManager;
use anyhow::Result;
use std::sync::Arc;

/// Start a reverse SSH tunnel: remote_port on the SSH host -> local_port on this machine
pub fn start_reverse_tunnel(
    _ssh_manager: Arc<SshManager>,
    remote_port: u16,
    local_port: u16,
) -> Result<()> {
    log::info!(
        "Starting reverse tunnel: remote:{} -> local:{}",
        remote_port,
        local_port
    );

    // The reverse tunnel uses SSH remote port forwarding.
    // In the full implementation, this maintains a persistent
    // SSH channel and forwards incoming connections on the remote
    // port to the local port.
    //
    // For now, this is a placeholder — the actual tunnel logic
    // requires a long-lived tokio task that:
    // 1. Listens for connections on remote port via ssh2
    // 2. Forwards them to 127.0.0.1:local_port

    log::info!("Reverse tunnel registered (remote:{} → local:{})", remote_port, local_port);

    Ok(())
}
