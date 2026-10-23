//! WireGuard X25519 keypairs, generated in-process.
//!
//! Deliberately not shelling out to `wg genkey`: a private key passed through a child process is
//! visible in the process table for as long as it runs, and requiring wireguard-tools just to
//! create a keypair is a dependency we do not need. The wire format is the same — base64 of the
//! 32 raw bytes — so these keys are interchangeable with anything wg produces.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::RngCore;
use x25519_dalek::{PublicKey, StaticSecret};

pub struct KeyPair {
    pub private: String,
    pub public: String,
}

pub fn generate() -> KeyPair {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    // StaticSecret clamps the scalar per RFC 7748, which is what wg genkey does too.
    let secret = StaticSecret::from(bytes);
    let public = PublicKey::from(&secret);
    KeyPair {
        private: STANDARD.encode(secret.to_bytes()),
        public: STANDARD.encode(public.to_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_32_bytes_of_base64() {
        let kp = generate();
        // 32 bytes base64-encodes to 44 chars ending in '='; wg rejects anything else.
        assert_eq!(kp.private.len(), 44, "private key wrong length");
        assert_eq!(kp.public.len(), 44, "public key wrong length");
        assert!(kp.private.ends_with('='));
        assert_eq!(STANDARD.decode(&kp.private).unwrap().len(), 32);
        assert_eq!(STANDARD.decode(&kp.public).unwrap().len(), 32);
    }

    #[test]
    fn pairs_are_unique_and_public_differs_from_private() {
        let a = generate();
        let b = generate();
        assert_ne!(a.private, b.private, "two calls produced the same private key");
        assert_ne!(a.private, a.public);
    }

    /// The one that matters: our public key must be the same one `wg pubkey` derives from our
    /// private key. If this drifts, every tunnel fails to hand shake with no useful error, because
    /// the server would be told to expect a peer that does not exist.
    ///
    /// Skipped when wireguard-tools is absent so the suite still runs on a bare CI box.
    #[test]
    fn public_key_matches_wg_pubkey() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let wg = ["/opt/homebrew/bin/wg", "/usr/local/bin/wg", "/usr/bin/wg"]
            .iter()
            .find(|p| std::path::Path::new(p).exists());
        let Some(wg) = wg else {
            eprintln!("wireguard-tools not installed; skipping compatibility check");
            return;
        };

        let kp = generate();
        let mut child = Command::new(wg)
            .arg("pubkey")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("could not run wg pubkey");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{}\n", kp.private).as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let from_wg = String::from_utf8_lossy(&out.stdout).trim().to_string();

        assert_eq!(
            from_wg, kp.public,
            "our public key disagrees with wg pubkey — the tunnel would never hand shake"
        );
    }
}
