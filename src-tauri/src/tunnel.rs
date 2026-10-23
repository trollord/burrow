//! Raising and lowering the tunnel.
//!
//! `wg-quick` has to run as root. Burrow asks for that through the system authorisation dialog
//! rather than making the user open a terminal — the same prompt any installer shows.
//!
//! This is the interim approach. The end state is a privileged helper registered once with
//! `SMAppService`, which costs one prompt at install and none afterwards; this costs one prompt
//! per connect. The trade was worth making now because the alternative was asking people to
//! copy-paste a sudo command, which is both worse and harder to trust.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Serialize, Clone, Debug)]
pub struct Status {
    /// A WireGuard interface carrying our tunnel address exists.
    pub up: bool,
    /// What the internet currently sees. Empty if it could not be checked.
    pub public_ip: String,
    /// public_ip matches the exit node, i.e. traffic really is going through it.
    pub tunnelled: bool,
}

pub fn wg_quick() -> Option<&'static str> {
    ["/opt/homebrew/bin/wg-quick", "/usr/local/bin/wg-quick", "/usr/bin/wg-quick"]
        .into_iter()
        .find(|p| Path::new(p).exists())
}

/// AppleScript string literals escape with backslashes, and the path we interpolate is
/// user-controlled only insofar as it is their home directory — but escaping is cheap and a
/// quote in a path would otherwise break the script in a confusing way.
fn osa_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn run_privileged(shell_cmd: &str) -> Result<String, String> {
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        osa_escape(shell_cmd)
    );
    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("could not ask for permission: {e}"))?;

    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    // -128 is the documented code for the user dismissing the dialog. Reporting that as a failure
    // would be wrong: nothing broke, they changed their mind.
    if err.contains("-128") || err.contains("User canceled") {
        Err("cancelled".to_string())
    } else {
        Err(err.trim().to_string())
    }
}

/// Is a tunnel up, and is traffic actually using it?
///
/// Deliberately does not need root. `wg show` would, but the question "is my traffic going through
/// the tunnel" is better answered by looking at the interface addresses and then at what the
/// internet reports back, which also catches the case where the interface exists but the handshake
/// never completed — a tunnel that is up and useless.
pub fn status(expected_endpoint: &str) -> Status {
    let up = Command::new("/sbin/ifconfig")
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .contains("inet 10.8.0.2")
        })
        .unwrap_or(false);

    let public_ip = Command::new("/usr/bin/curl")
        .args(["-4", "-fsS", "--max-time", "8", "https://api.ipify.org"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let tunnelled = !public_ip.is_empty()
        && !expected_endpoint.is_empty()
        && public_ip == expected_endpoint;

    Status { up, public_ip, tunnelled }
}

pub fn up(config: &Path) -> Result<(), String> {
    let wg = wg_quick().ok_or(
        "WireGuard isn't installed. Run: brew install wireguard-tools",
    )?;
    // wg-quick needs wg on PATH, and the shell osascript spawns has a minimal one.
    let dir = Path::new(wg).parent().map(|p| p.display().to_string()).unwrap_or_default();
    run_privileged(&format!(
        "export PATH={dir}:/usr/bin:/bin:/usr/sbin:/sbin; {wg} up '{}' 2>&1",
        config.display()
    ))
    .map(|_| ())
}

pub fn down(config: &Path) -> Result<(), String> {
    let wg = wg_quick().ok_or("WireGuard isn't installed.")?;
    let dir = Path::new(wg).parent().map(|p| p.display().to_string()).unwrap_or_default();
    run_privileged(&format!(
        "export PATH={dir}:/usr/bin:/bin:/usr/sbin:/sbin; {wg} down '{}' 2>&1",
        config.display()
    ))
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::osa_escape;

    #[test]
    fn escaping_protects_the_applescript_literal() {
        assert_eq!(osa_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(osa_escape(r"a\b"), r"a\\b");
        assert_eq!(osa_escape("/Users/x/.config/burrow/us-east-1.conf"),
                              "/Users/x/.config/burrow/us-east-1.conf");
    }
}
