//! Verification of the signed release manifest used by tunnel upgrades.
//!
//! The release workflow signs the exact bytes of `SHA256SUMS.txt`.  The
//! verifier deliberately does not parse or trust the manifest until the
//! detached signature has been checked against the embedded trust set.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

const MAX_ENVELOPE_BYTES: usize = 4096;
const MAX_KEY_ID_BYTES: usize = 128;
const ENVELOPE_VERSION: &str = "1";

/// Verify a release manifest against the public key configured at build time.
pub(crate) fn verify_release_manifest(manifest: &[u8], envelope: &[u8]) -> anyhow::Result<String> {
    let keys = embedded_trust_keys()?;
    verify_release_manifest_with_keys(manifest, envelope, &keys)
}

fn verify_release_manifest_with_keys(
    manifest: &[u8],
    envelope: &[u8],
    keys: &BTreeMap<String, VerifyingKey>,
) -> anyhow::Result<String> {
    if manifest.is_empty() {
        anyhow::bail!("signed release manifest is empty");
    }
    if envelope.is_empty() || envelope.len() > MAX_ENVELOPE_BYTES {
        anyhow::bail!("signed release envelope has an invalid size");
    }

    let mut fields = BTreeMap::new();
    let text = std::str::from_utf8(envelope)
        .map_err(|_| anyhow::anyhow!("signed release envelope is not UTF-8"))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            anyhow::bail!("signed release envelope contains an empty line");
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("signed release envelope has malformed fields"))?;
        if name.is_empty() || value.is_empty() || fields.insert(name, value).is_some() {
            anyhow::bail!("signed release envelope has duplicate or empty fields");
        }
    }
    if fields.len() != 3
        || fields
            .keys()
            .any(|key| !matches!(*key, "version" | "key_id" | "signature"))
    {
        anyhow::bail!("signed release envelope contains unknown fields");
    }

    if fields.get("version").copied() != Some(ENVELOPE_VERSION) {
        anyhow::bail!("signed release envelope has an unsupported version");
    }
    let key_id = fields
        .get("key_id")
        .copied()
        .ok_or_else(|| anyhow::anyhow!("signed release envelope is missing key_id"))?;
    if key_id.len() > MAX_KEY_ID_BYTES
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        anyhow::bail!("signed release envelope has an invalid key_id");
    }
    let signature_text = fields
        .get("signature")
        .copied()
        .ok_or_else(|| anyhow::anyhow!("signed release envelope is missing signature"))?;
    let signature_bytes = BASE64
        .decode(signature_text)
        .map_err(|_| anyhow::anyhow!("signed release envelope has an invalid signature"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| anyhow::anyhow!("signed release envelope has an invalid signature"))?;
    let key = keys
        .get(key_id)
        .ok_or_else(|| anyhow::anyhow!("signed release envelope references an unknown key"))?;
    key.verify(manifest, &signature)
        .map_err(|_| anyhow::anyhow!("signed release manifest signature verification failed"))?;
    Ok(key_id.to_string())
}

fn embedded_trust_keys() -> anyhow::Result<BTreeMap<String, VerifyingKey>> {
    let key_id = option_env!("AETHER_TUNNEL_RELEASE_KEY_ID")
        .ok_or_else(|| anyhow::anyhow!("tunnel release verifier has no trusted key"))?;
    let public_key = option_env!("AETHER_TUNNEL_RELEASE_PUBLIC_KEY")
        .ok_or_else(|| anyhow::anyhow!("tunnel release verifier has no trusted key"))?;
    let bytes = BASE64
        .decode(public_key)
        .map_err(|_| anyhow::anyhow!("tunnel release verifier has an invalid trusted key"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("tunnel release verifier has an invalid trusted key"))?;
    let key = VerifyingKey::from_bytes(&bytes)
        .map_err(|_| anyhow::anyhow!("tunnel release verifier has an invalid trusted key"))?;
    let mut keys = BTreeMap::new();
    keys.insert(key_id.to_string(), key);
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn fixture() -> (Vec<u8>, Vec<u8>, BTreeMap<String, VerifyingKey>) {
        let manifest = b"abc123  aether-tunnel-linux-amd64.tar.gz\n".to_vec();
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let signature = signing_key.sign(&manifest);
        let envelope = format!(
            "version=1\nkey_id=test-key\nsignature={}\n",
            BASE64.encode(signature.to_bytes())
        )
        .into_bytes();
        let mut keys = BTreeMap::new();
        keys.insert("test-key".to_string(), signing_key.verifying_key());
        (manifest, envelope, keys)
    }

    #[test]
    fn valid_signature_passes() {
        let (manifest, envelope, keys) = fixture();
        assert_eq!(
            verify_release_manifest_with_keys(&manifest, &envelope, &keys).unwrap(),
            "test-key"
        );
    }

    #[test]
    fn missing_signature_fails() {
        let (manifest, _, keys) = fixture();
        let envelope = b"version=1\nkey_id=test-key\n";
        assert!(verify_release_manifest_with_keys(&manifest, envelope, &keys).is_err());
    }

    #[test]
    fn unknown_key_fails() {
        let (manifest, envelope, _) = fixture();
        let keys = BTreeMap::new();
        assert!(verify_release_manifest_with_keys(&manifest, &envelope, &keys).is_err());
    }

    #[test]
    fn wrong_signature_fails() {
        let (mut manifest, envelope, keys) = fixture();
        manifest.push(b'x');
        assert!(verify_release_manifest_with_keys(&manifest, &envelope, &keys).is_err());
    }

    #[test]
    fn duplicate_and_unknown_fields_fail() {
        let (manifest, envelope, keys) = fixture();
        let duplicate = [envelope.as_slice(), b"version=1\n"].concat();
        assert!(verify_release_manifest_with_keys(&manifest, &duplicate, &keys).is_err());
        let unknown = envelope
            .iter()
            .chain(b"extra=x\n".iter())
            .copied()
            .collect::<Vec<_>>();
        assert!(verify_release_manifest_with_keys(&manifest, &unknown, &keys).is_err());
    }

    #[test]
    fn malformed_key_and_signature_lengths_fail() {
        let (manifest, _, keys) = fixture();
        let envelope = b"version=1\nkey_id=test-key\nsignature=AA==\n";
        assert!(verify_release_manifest_with_keys(&manifest, envelope, &keys).is_err());
        let envelope = b"version=1\nkey_id=test-key\nsignature=%%%\n";
        assert!(verify_release_manifest_with_keys(&manifest, envelope, &keys).is_err());
    }
}
