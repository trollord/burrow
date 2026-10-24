//! Client for the privileged helper, and its one-time installation.
//!
//! With the helper installed, connecting costs nothing: the app writes a line to a socket and a
//! root daemon does the work. Without it, `tunnel.rs` falls back to prompting for admin on every
//! connect, which is what this exists to remove.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub const SOCKET: &str = "/var/run/burrow-helper.sock";
const LABEL: &str = "io.github.trollord.burrow.helper";
const INSTALL_DIR: &str = "/Library/Application Support/Burrow";
const HELPER_BIN: &str = "/Library/PrivilegedHelperTools/io.github.trollord.burrow.helper";
const PLIST: &str = "/Library/LaunchDaemons/io.github.trollord.burrow.helper.plist";

/// Is the helper installed and answering?
pub fn available() -> bool {
    talk("{\"action\":\"ping\"}").is_ok()
}

pub fn installed() -> bool {
    Path::new(HELPER_BIN).exists() && Path::new(PLIST).exists()
}

/// Send one request and read one reply.
fn talk(request: &str) -> Result<String, String> {
    let mut stream = UnixStream::connect(SOCKET)
        .map_err(|e| format!("helper is not running: {e}"))?;
    // A hung daemon must not hang the UI. wg-quick on a slow link can take a few seconds, so the
    // read timeout is generous while still bounded.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(45)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|e| format!("could not reach the helper: {e}"))?;
    stream.flush().ok();

    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| format!("no reply from the helper: {e}"))?;

    if line.contains("\"ok\":true") {
        Ok(line)
    } else {
        // Surface the daemon's own message rather than a generic failure; it is usually the
        // actual wg-quick error.
        let msg = line
            .split("\"message\":\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("the helper refused the request")
            .to_string();
        Err(msg)
    }
}

pub fn install_config(
    region: &str,
    private_key: &str,
    peer_key: &str,
    endpoint: &str,
) -> Result<(), String> {
    talk(&format!(
        "{{\"action\":\"install\",\"region\":\"{region}\",\"private_key\":\"{private_key}\",\"peer_key\":\"{peer_key}\",\"endpoint\":\"{endpoint}\"}}"
    ))
    .map(|_| ())
}

pub fn up(region: &str) -> Result<(), String> {
    talk(&format!("{{\"action\":\"up\",\"region\":\"{region}\"}}")).map(|_| ())
}

pub fn down(region: &str) -> Result<(), String> {
    talk(&format!("{{\"action\":\"down\",\"region\":\"{region}\"}}")).map(|_| ())
}

pub fn remove(region: &str) -> Result<(), String> {
    talk(&format!("{{\"action\":\"remove\",\"region\":\"{region}\"}}")).map(|_| ())
}

/// Where the helper binary ships inside the app bundle.
///
/// Tauri places external binaries beside the main executable in `Contents/MacOS`, so it gets
/// signed with the same identity as the app — which is what lets the daemon be trusted at all.
fn bundled_helper() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no parent directory")?;
    for name in ["burrow-helper", "burrow-helper-aarch64-apple-darwin", "burrow-helper-x86_64-apple-darwin"] {
        let p = dir.join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err("the helper binary is missing from the app bundle".into())
}

/// Install the daemon. Prompts for admin exactly once, ever.
///
/// Everything privileged happens inside a single `osascript` call, so one authorisation covers
/// the copy, the plist and the launchctl load. Doing them as separate privileged calls would
/// prompt three times and defeat the purpose.
pub fn install() -> Result<(), String> {
    let src = bundled_helper()?;
    let uid = unsafe { libc::getuid() };

    // The daemon is KeepAlive so it comes back after a crash and starts at boot; the socket is
    // recreated on each launch. RunAtLoad means the first connect does not race the install.
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{HELPER_BIN}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/var/log/burrow-helper.log</string>
</dict>
</plist>"#
    );

    let script = format!(
        "set -e; \
         mkdir -p /Library/PrivilegedHelperTools '{INSTALL_DIR}'; \
         cp '{src}' '{HELPER_BIN}'; \
         chown root:wheel '{HELPER_BIN}'; \
         chmod 544 '{HELPER_BIN}'; \
         printf '%s' '{uid}' > '{INSTALL_DIR}/owner'; \
         chmod 644 '{INSTALL_DIR}/owner'; \
         cat > '{PLIST}' <<'PLISTEOF'\n{plist}\nPLISTEOF\n\
         chown root:wheel '{PLIST}'; chmod 644 '{PLIST}'; \
         launchctl bootout system '{PLIST}' 2>/dev/null || true; \
         launchctl bootstrap system '{PLIST}'",
        src = src.display()
    );

    crate::tunnel::run_privileged(&script)?;

    // launchd needs a moment to start it and bind the socket.
    for _ in 0..20 {
        if available() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("the helper was installed but did not start. Check /var/log/burrow-helper.log".into())
}

/// Remove the daemon entirely. Also one prompt.
pub fn uninstall() -> Result<(), String> {
    crate::tunnel::run_privileged(&format!(
        "launchctl bootout system '{PLIST}' 2>/dev/null || true; \
         rm -f '{PLIST}' '{HELPER_BIN}' '{SOCKET}'; \
         rm -rf '{INSTALL_DIR}/tunnels'"
    ))
    .map(|_| ())
}
