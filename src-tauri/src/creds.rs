//! AWS credential storage and validation.
//!
//! Credentials go in the OS keychain, never in a config file. Burrow is asking people to paste
//! keys that can spend money on their account, so the one thing it must not do is leave them in
//! plaintext somewhere a backup or a stray `cat` will find.

use aws_credential_types::Credentials;
use serde::Serialize;

const SERVICE: &str = "co.bytelights.burrow";
const ACCOUNT: &str = "aws";

/// What the UI needs to know about a stored credential. Deliberately contains no secret: the
/// access key id is shown so people can tell which key is in use, the secret never leaves here.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Identity {
    pub account: String,
    pub arn: String,
    pub access_key_id: String,
    pub lightsail_ok: bool,
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("keychain unavailable: {e}"))
}

/// Stored as "AKIA...:secret". The access key id is not secret, so a single keychain item holding
/// both is simpler than two items that could drift out of sync.
pub fn save(access_key_id: &str, secret: &str) -> Result<(), String> {
    entry()?
        .set_password(&format!("{access_key_id}:{secret}"))
        .map_err(|e| format!("could not write to the keychain: {e}"))
}

pub fn load() -> Result<Option<(String, String)>, String> {
    match entry()?.get_password() {
        Ok(v) => match v.split_once(':') {
            Some((id, secret)) if !id.is_empty() && !secret.is_empty() => {
                Ok(Some((id.to_string(), secret.to_string())))
            }
            // A malformed item is treated as absent rather than as an error: the user can simply
            // re-enter their key instead of being stuck behind a message they cannot act on.
            _ => Ok(None),
        },
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("could not read the keychain: {e}")),
    }
}

pub fn clear() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("could not clear the keychain: {e}")),
    }
}

/// Turn a key pair into an SDK config for a given region.
pub async fn config_for(
    access_key_id: &str,
    secret: &str,
    region: &str,
) -> aws_config::SdkConfig {
    let creds = Credentials::new(access_key_id, secret, None, None, "burrow");
    aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .credentials_provider(creds)
        .load()
        .await
}

/// Prove the keys work AND that they can actually drive Lightsail.
///
/// Both halves matter. STS succeeding only says the key is real; plenty of IAM users authenticate
/// fine and then fail at the first Lightsail call because nobody attached the policy. Finding that
/// out during onboarding, with a message naming the exact policy, beats finding out halfway
/// through provisioning.
pub async fn verify(access_key_id: &str, secret: &str, region: &str) -> Result<Identity, String> {
    let config = config_for(access_key_id, secret, region).await;

    let who = aws_sdk_sts::Client::new(&config)
        .get_caller_identity()
        .send()
        .await
        .map_err(|e| {
            let msg = format!("{}", aws_sdk_sts::error::DisplayErrorContext(&e));
            if msg.contains("InvalidClientTokenId") || msg.contains("SignatureDoesNotMatch") {
                "Those credentials were rejected by AWS. Check for a missing character \
                 — secrets are 40 characters and easy to truncate when copying."
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
        access_key_id: access_key_id.to_string(),
        lightsail_ok,
    })
}
