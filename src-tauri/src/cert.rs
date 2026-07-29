/// Trust the bundled bridge.crt by installing it into the OS cert store.
/// This allows the frontend's WebSocket (wss://) to connect without cert errors.
const BRIDGE_CERT: &[u8] = include_bytes!("../resources/bridge.crt");

pub fn trust_bridge_cert() {
    #[cfg(target_os = "windows")]
    {
        let tmp = std::env::temp_dir().join("retana-bridge.crt");
        if std::fs::write(&tmp, BRIDGE_CERT).is_err() {
            log::warn!("Failed to write bridge cert to temp dir");
            return;
        }
        match std::process::Command::new("certutil")
            .args(["-addstore", "-user", "Root"])
            .arg(&tmp)
            .output()
        {
            Ok(out) if out.status.success() => {
                log::info!("✅ Bridge cert added to Windows trust store");
            }
            Ok(out) => {
                log::warn!("certutil failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            Err(e) => {
                log::warn!("certutil not found: {}", e);
            }
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[cfg(target_os = "macos")]
    {
        let tmp = std::env::temp_dir().join("retana-bridge.crt");
        if std::fs::write(&tmp, BRIDGE_CERT).is_err() {
            log::warn!("Failed to write bridge cert to temp dir");
            return;
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let keychain = format!("{}/Library/Keychains/login.keychain-db", home);
        match std::process::Command::new("security")
            .args(["add-trusted-cert", "-d", "-r", "trustRoot", "-k", &keychain])
            .arg(&tmp)
            .output()
        {
            Ok(out) if out.status.success() => {
                log::info!("✅ Bridge cert added to macOS keychain");
            }
            Ok(out) => {
                log::info!(
                    "security add-trusted-cert: {} (may already be trusted)",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Err(e) => {
                log::warn!("security tool failed: {}", e);
            }
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[cfg(target_os = "linux")]
    {
        log::info!(
            "Cert trust: run 'sudo cp bridge.crt /usr/local/share/ca-certificates/ && sudo update-ca-certificates'"
        );
    }
}
