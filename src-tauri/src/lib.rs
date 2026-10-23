mod creds;
mod keys;
mod node;
mod tunnel;

use serde::Serialize;
use std::path::PathBuf;

/// Locations offered in the picker. Lightsail has 19 regions; these are the ones people actually
/// want for a VPN. Adding more is a line each — bundle and availability zone are both discovered
/// per region at provision time, so nothing else needs touching.
const REGIONS: &[(&str, &str, &str)] = &[
    ("us-east-1", "Virginia", "United States"),
    ("us-east-2", "Ohio", "United States"),
    ("us-west-2", "Oregon", "United States"),
    ("eu-central-1", "Frankfurt", "Europe"),
    ("eu-west-2", "London", "Europe"),
    ("eu-west-1", "Ireland", "Europe"),
    ("ap-southeast-2", "Sydney", "Australia"),
];

const DEFAULT_DNS: &str = "1.1.1.1, 1.0.0.1";

#[derive(Serialize)]
struct Region {
    id: &'static str,
    label: &'static str,
    group: &'static str,
}

#[tauri::command]
fn regions() -> Vec<Region> {
    REGIONS
        .iter()
        .map(|(id, label, group)| Region { id, label, group })
        .collect()
}

/// What this location costs, read live from AWS.
///
/// Prices and especially transfer allowances are not uniform: Sydney charges the same $5 for half
/// the data as Virginia, and the bundle is called something different there. Showing the real
/// numbers before someone commits is worth the extra round trip.
#[tauri::command]
async fn region_plan(region: String) -> Result<node::Plan, String> {
    let (id, secret) = creds::load()?.ok_or("no credentials saved")?;
    let config = creds::config_for(&id, &secret, &region).await;
    node::plan_for(&aws_sdk_lightsail::Client::new(&config)).await
}

/// Configs live outside the app bundle and outside any synced folder.
///
/// On macOS this also dodges TCC: ~/Desktop, ~/Documents and ~/Downloads are protected, and a root
/// process raised to run wg-quick gets "Operation not permitted" reading a config in any of them —
/// a failure that looks like a WireGuard bug and is not.
fn config_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME set".to_string())?;
    let dir = PathBuf::from(home).join(".config").join("burrow");
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {dir:?}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

fn config_path(region: &str) -> Result<PathBuf, String> {
    Ok(config_dir()?.join(format!("{region}.conf")))
}

#[derive(Serialize)]
struct Setup {
    has_credentials: bool,
    identity: Option<creds::Identity>,
    wireguard_installed: bool,
}

/// What the UI needs on launch to decide which screen to show.
#[tauri::command]
async fn setup_state() -> Result<Setup, String> {
    let stored = creds::load()?;
    let wireguard_installed = which_wg().is_some();
    match stored {
        None => Ok(Setup {
            has_credentials: false,
            identity: None,
            wireguard_installed,
        }),
        Some((id, secret)) => {
            // Verify rather than trust: a key valid last week may have been revoked since, and
            // discovering that at connect time is a worse experience than at launch.
            let identity = creds::verify(&id, &secret, REGIONS[0].0).await.ok();
            Ok(Setup {
                has_credentials: true,
                identity,
                wireguard_installed,
            })
        }
    }
}

#[tauri::command]
async fn save_credentials(access_key_id: String, secret: String) -> Result<creds::Identity, String> {
    let access_key_id = access_key_id.trim().to_string();
    let secret = secret.trim().to_string();

    // Catch the two mistakes people actually make — pasting the wrong field, and truncating the
    // secret — with a message that says which, rather than letting AWS answer "invalid token".
    if !access_key_id.starts_with("AKIA") && !access_key_id.starts_with("ASIA") {
        return Err("That does not look like an Access key ID — they begin with AKIA.".into());
    }
    if secret.len() != 40 {
        return Err(format!(
            "A secret access key is 40 characters; this one is {}. It was probably truncated when copying.",
            secret.len()
        ));
    }

    let identity = creds::verify(&access_key_id, &secret, REGIONS[0].0).await?;
    if !identity.lightsail_ok {
        return Err(
            "Those credentials work, but they cannot use Lightsail. Attach the \
             AmazonLightsailFullAccess policy to this IAM user and try again."
                .into(),
        );
    }
    creds::save(&access_key_id, &secret)?;
    Ok(identity)
}

#[tauri::command]
fn forget_credentials() -> Result<(), String> {
    creds::clear()
}

#[tauri::command]
async fn node_status(region: String) -> Result<Option<node::Node>, String> {
    let (id, secret) = creds::load()?.ok_or("no credentials saved")?;
    let config = creds::config_for(&id, &secret, &region).await;
    Ok(node::find(&aws_sdk_lightsail::Client::new(&config), &region).await)
}

#[tauri::command]
async fn create_node(region: String) -> Result<node::Node, String> {
    let (id, secret) = creds::load()?.ok_or("no credentials saved")?;
    let result = node::provision(&id, &secret, &region, DEFAULT_DNS).await?;

    let path = config_path(&region)?;
    std::fs::write(&path, &result.config).map_err(|e| format!("could not write the config: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("could not secure the config file: {e}"))?;
    }
    Ok(result.node)
}

#[tauri::command]
async fn destroy_node(region: String) -> Result<Vec<String>, String> {
    let (id, secret) = creds::load()?.ok_or("no credentials saved")?;
    let steps = node::destroy(&id, &secret, &region).await?;
    let _ = std::fs::remove_file(config_path(&region)?);
    Ok(steps)
}

fn which_wg() -> Option<String> {
    tunnel::wg_quick().map(|s| s.to_string())
}

#[tauri::command]
fn config_file_path(region: String) -> Result<String, String> {
    Ok(config_path(&region)?.to_string_lossy().to_string())
}

/// Raise the tunnel. Prompts for admin once, via the system dialog.
#[tauri::command]
async fn connect(region: String) -> Result<tunnel::Status, String> {
    let path = config_path(&region)?;
    if !path.exists() {
        return Err("No configuration for this location — create the server first.".into());
    }
    tunnel::up(&path)?;

    // Coming back from wg-quick only means the interface was created. Whether traffic actually
    // leaves through the exit node is a different question, and the one worth answering: poll
    // until the public address matches, rather than claiming success on an interface that exists
    // but never handshook.
    let endpoint = node_endpoint(&region).await.unwrap_or_default();
    for _ in 0..10 {
        let st = tunnel::status(&endpoint);
        if st.tunnelled {
            return Ok(st);
        }
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    }
    Err("The tunnel came up but traffic is not going through it yet. \
         Give the server another moment, then try again.".into())
}

#[tauri::command]
async fn disconnect(region: String) -> Result<tunnel::Status, String> {
    tunnel::down(&config_path(&region)?)?;
    Ok(tunnel::status(""))
}

/// Current tunnel state, cheap enough to poll from the UI.
#[tauri::command]
async fn tunnel_status(region: String) -> Result<tunnel::Status, String> {
    let endpoint = node_endpoint(&region).await.unwrap_or_default();
    Ok(tunnel::status(&endpoint))
}

async fn node_endpoint(region: &str) -> Option<String> {
    let (id, secret) = creds::load().ok()??;
    let config = creds::config_for(&id, &secret, region).await;
    node::find(&aws_sdk_lightsail::Client::new(&config), region)
        .await
        .map(|n| n.endpoint)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            regions,
            setup_state,
            save_credentials,
            forget_credentials,
            region_plan,
            node_status,
            create_node,
            destroy_node,
            config_file_path,
            connect,
            disconnect,
            tunnel_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
