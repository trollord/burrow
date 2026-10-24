//! Named AWS profiles.
//!
//! Secrets go in the OS keychain; everything else — the name you gave it, the access key id, the
//! account it belongs to — goes in a plain JSON index beside the configs. The split matters for
//! one practical reason: **listing your profiles never touches the keychain**, so opening the app
//! does not trigger an authorisation prompt. Only an action that actually calls AWS does.
//!
//! An access key id is not a secret (it appears in logs and ARNs), so holding it in the index is
//! what lets the UI show you which key is which without unlocking anything.

use crate::paths::{config_dir, now, read_json, write_json};
use aws_credential_types::Credentials;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const SERVICE: &str = "io.github.trollord.burrow";

/// Secrets already read this run, by profile name.
///
/// Without this the keychain is read once per AWS call — and the location screen alone makes
/// seven, one per region, which macOS presents as seven authorisation prompts in a row. One read
/// per profile per run is both the correct number and the difference between a usable app and an
/// unusable one.
///
/// The secret sits in process memory for the lifetime of the app. That is unavoidable: the AWS
/// SDK needs it in memory to sign every request anyway, so caching changes how long it is held,
/// not whether it is held.
fn cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(name: &str) -> Option<String> {
    cache().lock().ok()?.get(name).cloned()
}

fn remember(name: &str, secret: &str) {
    if let Ok(mut c) = cache().lock() {
        c.insert(name.to_string(), secret.to_string());
    }
}

fn forget(name: &str) {
    if let Ok(mut c) = cache().lock() {
        c.remove(name);
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    /// Whatever the user called it: "personal", "work", "client acme".
    pub name: String,
    pub access_key_id: String,
    pub account: String,
    pub arn: String,
    pub added_at: i64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Store {
    pub active: Option<String>,
    pub profiles: Vec<Profile>,
}

fn index_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("profiles.json"))
}

fn entry(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, name).map_err(|e| format!("keychain unavailable: {e}"))
}

/// Every saved profile, and which is selected. Reads one small JSON file; no keychain, no prompt.
pub fn list() -> Store {
    index_path().map(|p| read_json(&p)).unwrap_or_default()
}

pub fn active() -> Option<Profile> {
    let store = list();
    let name = store.active.as_ref()?;
    store.profiles.iter().find(|p| &p.name == name).cloned()
}

/// Access key and secret for the active profile. This is the call that may prompt.
pub fn active_credentials() -> Result<(String, String), String> {
    let p = active().ok_or("No AWS profile selected.")?;
    if let Some(secret) = cached(&p.name) {
        return Ok((p.access_key_id, secret));
    }
    let secret = entry(&p.name)?
        .get_password()
        .map_err(|_| format!("The key for \"{}\" is missing from the keychain. Add it again.", p.name))?;
    remember(&p.name, &secret);
    Ok((p.access_key_id, secret))
}

/// Verify a key, then save it under a name. Verification happens first so a bad key is never
/// stored and never has to be cleaned up.
pub async fn add(name: &str, access_key_id: &str, secret: &str, region: &str) -> Result<Profile, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give this key a name so you can tell it apart later.".into());
    }
    let identity = verify(access_key_id, secret, region).await?;
    if !identity.lightsail_ok {
        return Err(
            "Those credentials work, but they cannot use Lightsail. Attach the \
             AmazonLightsailFullAccess policy to this IAM user and try again."
                .into(),
        );
    }

    entry(name)?
        .set_password(secret)
        .map_err(|e| format!("could not write to the keychain: {e}"))?;
    // Already in hand, so seed the cache: adding a key should never be followed by a prompt to
    // read back the very thing just written.
    remember(name, secret);

    let mut store = list();
    let profile = Profile {
        name: name.to_string(),
        access_key_id: access_key_id.to_string(),
        account: identity.account,
        arn: identity.arn,
        added_at: now(),
    };
    // Re-adding under an existing name replaces it rather than making a duplicate, which is what
    // someone rotating a key expects to happen.
    store.profiles.retain(|p| p.name != profile.name);
    store.profiles.push(profile.clone());
    store.active = Some(profile.name.clone());
    write_json(&index_path()?, &store)?;
    Ok(profile)
}

pub fn select(name: &str) -> Result<(), String> {
    let mut store = list();
    if !store.profiles.iter().any(|p| p.name == name) {
        return Err(format!("No profile called \"{name}\"."));
    }
    store.active = Some(name.to_string());
    write_json(&index_path()?, &store)
}

pub fn remove(name: &str) -> Result<(), String> {
    forget(name);
    // Drop the keychain item too, or the secret outlives the profile with nothing pointing at it.
    if let Ok(e) = entry(name) {
        let _ = e.delete_credential();
    }
    let mut store = list();
    store.profiles.retain(|p| p.name != name);
    if store.active.as_deref() == Some(name) {
        store.active = store.profiles.first().map(|p| p.name.clone());
    }
    write_json(&index_path()?, &store)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Identity {
    pub account: String,
    pub arn: String,
    pub lightsail_ok: bool,
}

pub async fn config_for(access_key_id: &str, secret: &str, region: &str) -> aws_config::SdkConfig {
    let creds = Credentials::new(access_key_id, secret, None, None, "burrow");
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .credentials_provider(creds)
        .load()
        .await
}

/// SDK config for whichever profile is active.
pub async fn active_config(region: &str) -> Result<aws_config::SdkConfig, String> {
    let (id, secret) = active_credentials()?;
    Ok(config_for(&id, &secret, region).await)
}

/// Prove a key works AND that it can drive Lightsail.
///
/// Both halves matter. STS succeeding only says the key is real; plenty of IAM users authenticate
/// and then fail at the first Lightsail call because nobody attached the policy. Catching that
/// during setup, with a message naming the policy, beats failing mid-provision.
pub async fn verify(access_key_id: &str, secret: &str, region: &str) -> Result<Identity, String> {
    let config = config_for(access_key_id, secret, region).await;

    let who = aws_sdk_sts::Client::new(&config)
        .get_caller_identity()
        .send()
        .await
        .map_err(|e| {
            let msg = format!("{}", aws_sdk_sts::error::DisplayErrorContext(&e));
            if msg.contains("InvalidClientTokenId") || msg.contains("SignatureDoesNotMatch") {
                "AWS rejected those credentials. Check for a missing character — secrets are \
                 40 characters and easy to truncate when copying."
                    .to_string()
            } else {
                format!("AWS rejected the credentials: {msg}")
            }
        })?;

    let lightsail_ok = aws_sdk_lightsail::Client::new(&config)
        .get_regions()
        .send()
        .await
        .is_ok();

    Ok(Identity {
        account: who.account().unwrap_or_default().to_string(),
        arn: who.arn().unwrap_or_default().to_string(),
        lightsail_ok,
    })
}
