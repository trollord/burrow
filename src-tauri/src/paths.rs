//! Where Burrow keeps its files.

use std::path::PathBuf;

/// `~/.config/burrow`, created if absent and locked to the owner on Unix.
///
/// Two reasons it is not somewhere more obvious like the Desktop. It holds WireGuard configs,
/// which contain a private key, so it stays out of any directory people habitually share or sync.
/// And on macOS, `~/Desktop`, `~/Documents` and `~/Downloads` are TCC-protected: a root process
/// raised to run `wg-quick` gets "Operation not permitted" reading a config from any of them,
/// which surfaces as a WireGuard error that has nothing to do with WireGuard.
pub fn config_dir() -> Result<PathBuf, String> {
    // HOME on Unix, USERPROFILE on Windows. Reading only HOME meant Windows failed here — after
    // a billable server had already been created, which is the worst possible place to fail.
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "could not find your home directory".to_string())?;
    let dir = PathBuf::from(home).join(".config").join("burrow");
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

pub fn config_file(region: &str) -> Result<PathBuf, String> {
    Ok(config_dir()?.join(format!("{region}.conf")))
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// Read a JSON file, returning the default when it is missing or unreadable.
///
/// Corrupt local state should never be fatal: the worst case is a forgotten profile list, and
/// making someone hand-edit JSON to open an app would be worse than starting fresh.
pub fn read_json<T: serde::de::DeserializeOwned + Default>(path: &PathBuf) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn write_json<T: serde::Serialize>(path: &PathBuf, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}
