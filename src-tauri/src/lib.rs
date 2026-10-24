mod creds;
mod keys;
mod node;
mod paths;
mod sessions;
mod tunnel;

use paths::config_file;
use serde::Serialize;

/// Locations offered in the picker. Lightsail has 19 regions; these are the ones people want for
/// a VPN. Adding more is a line each — instance size and availability zone are discovered per
/// region at provision time, so nothing else needs touching.
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

// ── profiles ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Startup {
    profiles: Vec<creds::Profile>,
    active: Option<String>,
    wireguard_installed: bool,
}

/// What the UI needs on launch.
///
/// Deliberately reads only the profile index, never the keychain: touching the keychain here is
/// what threw an authorisation dialog in your face before the app had even drawn. Nothing needs
/// a secret until you ask it to do something with AWS.
#[tauri::command]
fn startup() -> Startup {
    let store = creds::list();
    Startup {
        profiles: store.profiles,
        active: store.active,
        wireguard_installed: tunnel::wg_quick().is_some(),
    }
}

#[tauri::command]
async fn add_profile(
    name: String,
    access_key_id: String,
    secret: String,
) -> Result<creds::Profile, String> {
    let access_key_id = access_key_id.trim().to_string();
    let secret = secret.trim().to_string();

    // Catch the two mistakes people actually make — pasting the wrong field, and truncating the
    // secret — rather than letting AWS answer with "invalid token".
    if !access_key_id.starts_with("AKIA") && !access_key_id.starts_with("ASIA") {
        return Err("That does not look like an Access key ID — they begin with AKIA.".into());
    }
    if secret.len() != 40 {
        return Err(format!(
            "A secret access key is 40 characters; this one is {}. It was probably truncated when copying.",
            secret.len()
        ));
    }
    creds::add(&name, &access_key_id, &secret, REGIONS[0].0).await
}

#[tauri::command]
fn select_profile(name: String) -> Result<(), String> {
    creds::select(&name)
}

#[tauri::command]
fn remove_profile(name: String) -> Result<(), String> {
    creds::remove(&name)
}

// ── servers ───────────────────────────────────────────────────────────────

#[tauri::command]
async fn region_plan(region: String) -> Result<node::Plan, String> {
    let config = creds::active_config(&region).await?;
    node::plan_for(&aws_sdk_lightsail::Client::new(&config)).await
}

/// Every running server, across all offered regions, checked concurrently.
///
/// The credentials are read from the keychain **once** and reused for each region. Calling
/// `active_config` per region would read it seven times, and on an unsigned build that is seven
/// authorisation prompts in a row.
#[tauri::command]
async fn all_nodes() -> Result<Vec<node::Node>, String> {
    let (key, secret) = creds::active_credentials()?;
    let mut handles = Vec::new();
    for (region, _, _) in REGIONS {
        let (key, secret, region) = (key.clone(), secret.clone(), region.to_string());
        handles.push(tokio::spawn(async move {
            let cfg = creds::config_for(&key, &secret, &region).await;
            node::find(&aws_sdk_lightsail::Client::new(&cfg), &region).await
        }));
    }
    let mut found = Vec::new();
    for h in handles {
        // A region that errors is simply not where a server is; one unreachable region must not
        // hide the others.
        if let Ok(Some(n)) = h.await {
            found.push(n);
        }
    }
    Ok(found)
}

#[tauri::command]
async fn node_status(region: String) -> Result<Option<node::Node>, String> {
    let config = creds::active_config(&region).await?;
    Ok(node::find(&aws_sdk_lightsail::Client::new(&config), &region).await)
}

#[tauri::command]
async fn create_node(region: String) -> Result<node::Node, String> {
    let (id, secret) = creds::active_credentials()?;
    let result = node::provision(&id, &secret, &region, DEFAULT_DNS).await?;

    let path = config_file(&region)?;
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
    let (id, secret) = creds::active_credentials()?;
    let steps = node::destroy(&id, &secret, &region).await?;
    let _ = std::fs::remove_file(config_file(&region)?);
    Ok(steps)
}

// ── tunnel ────────────────────────────────────────────────────────────────

#[tauri::command]
async fn connect(region: String) -> Result<tunnel::Status, String> {
    let path = config_file(&region)?;
    if !path.exists() {
        return Err("No configuration for this location — create the server first.".into());
    }
    tunnel::up(&path)?;

    // Returning from wg-quick only means an interface was created. Whether traffic actually
    // leaves through the exit node is the question worth answering, so poll until the public
    // address matches rather than declaring success on an interface that never handshook.
    let endpoint = node_endpoint(&region).await.unwrap_or_default();
    for _ in 0..10 {
        let st = tunnel::status(&endpoint);
        if st.tunnelled {
            let _ = sessions::begin(&region, &endpoint);
            return Ok(st);
        }
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    }
    Err("The tunnel came up but traffic is not going through it yet. \
         Give the server another moment, then try again."
        .into())
}

#[tauri::command]
async fn disconnect(region: String) -> Result<tunnel::Status, String> {
    // Read the counters BEFORE tearing the interface down — they vanish with it.
    let _ = sessions::end();
    tunnel::down(&config_file(&region)?)?;
    Ok(tunnel::status(""))
}

#[tauri::command]
async fn tunnel_status(region: String) -> Result<tunnel::Status, String> {
    let endpoint = node_endpoint(&region).await.unwrap_or_default();
    Ok(tunnel::status(&endpoint))
}

async fn node_endpoint(region: &str) -> Option<String> {
    let config = creds::active_config(region).await.ok()?;
    node::find(&aws_sdk_lightsail::Client::new(&config), region)
        .await
        .map(|n| n.endpoint)
}

// ── sessions ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Usage {
    live: Option<sessions::Session>,
    recent: Vec<sessions::Session>,
    month: sessions::Totals,
}

/// Live usage plus history. Cheap — two small file reads and one `netstat`.
#[tauri::command]
fn usage() -> Usage {
    let live = sessions::sample().ok().flatten();
    // Thirty days rather than a calendar month: the Lightsail allowance accrues against the
    // instance's own billing cycle, which does not begin on the 1st.
    let cutoff = paths::now() - 30 * 24 * 3600;
    Usage {
        live,
        recent: sessions::history(20),
        month: sessions::totals_since(cutoff),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            regions,
            startup,
            add_profile,
            select_profile,
            remove_profile,
            region_plan,
            all_nodes,
            node_status,
            create_node,
            destroy_node,
            connect,
            disconnect,
            tunnel_status,
            usage,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
