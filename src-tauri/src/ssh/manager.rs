// SSH connection manager
// Manages persistent SSH connections to remote Hermes instances

use anyhow::{Context, Result};
use ssh2::Session;
use std::io::Read;
use std::net::TcpStream;
use std::sync::Mutex;

/// SSH connection configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Path to private key file, or None for password auth
    pub key_path: Option<String>,
    pub password: Option<String>,
    /// Reverse tunnel: remote_port -> local_port
    pub reverse_tunnel: Option<ReverseTunnelConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReverseTunnelConfig {
    pub remote_port: u16,
    pub local_port: u16,
}

/// Connection status
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// Managed SSH connection
pub struct SshManager {
    config: SshConfig,
    pub(crate) session: Mutex<Option<Session>>,
    status: Mutex<ConnectionStatus>,
    tcp_stream: Mutex<Option<TcpStream>>,
}

impl SshManager {
    pub fn new(config: SshConfig) -> Self {
        Self {
            config,
            session: Mutex::new(None),
            status: Mutex::new(ConnectionStatus::Disconnected),
            tcp_stream: Mutex::new(None),
        }
    }

    /// Connect to the remote host
    pub fn connect(&self) -> Result<()> {
        *self.status.lock().unwrap() = ConnectionStatus::Connecting;

        let addr = format!("{}:{}", self.config.host, self.config.port);
        let tcp = TcpStream::connect(&addr)
            .with_context(|| format!("Failed to connect to {}", addr))?;

        let mut session = Session::new()
            .context("Failed to create SSH session")?;
        session.set_tcp_stream(tcp.try_clone()?);
        session.handshake()
            .context("SSH handshake failed")?;

        // Authenticate
        if let Some(ref key_path) = self.config.key_path {
            session.userauth_pubkey_file(
                &self.config.username,
                None,
                std::path::Path::new(key_path),
                None,
            ).context("Public key authentication failed")?;
        } else if let Some(ref password) = self.config.password {
            session.userauth_password(&self.config.username, password)
                .context("Password authentication failed")?;
        } else {
            anyhow::bail!("No authentication method provided (key_path or password)");
        }

        if !session.authenticated() {
            anyhow::bail!("SSH authentication failed");
        }

        *self.session.lock().unwrap() = Some(session);
        *self.tcp_stream.lock().unwrap() = Some(tcp);
        *self.status.lock().unwrap() = ConnectionStatus::Connected;

        Ok(())
    }

    /// Execute a command on the remote host and return stdout
    pub fn exec(&self, command: &str) -> Result<String> {
        let session_guard = self.session.lock().unwrap();
        let session = session_guard.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        let mut channel = session.channel_session()
            .context("Failed to create channel")?;
        channel.exec(command)
            .context("Failed to execute command")?;

        let mut output = String::new();
        channel.read_to_string(&mut output)
            .context("Failed to read command output")?;

        channel.wait_close()?;
        Ok(output)
    }

    /// Get current connection status
    pub fn status(&self) -> ConnectionStatus {
        self.status.lock().unwrap().clone()
    }

    /// Disconnect from the remote host
    pub fn disconnect(&self) -> Result<()> {
        let mut session = self.session.lock().unwrap();
        let mut tcp = self.tcp_stream.lock().unwrap();
        *session = None;
        *tcp = None;
        *self.status.lock().unwrap() = ConnectionStatus::Disconnected;
        Ok(())
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        *self.status.lock().unwrap() == ConnectionStatus::Connected
    }
}
