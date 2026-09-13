//! Verification of the signed release manifest used by tunnel upgrades.
//!
//! The release workflow signs the exact bytes of `SHA256SUMS.txt`.  The
//! verifier deliberately does not parse or trust the manifest until the
//! detached signature has been checked against the embedded trust set.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;

const MAX_ENVELOPE_BYTES: usize = 4096;
const MAX_KEY_ID_BYTES: usize = 128;
const ENVELOPE_VERSION: &str = "1";
const MAX_TRUST_KEYS: usize = 16;
const MAX_TRUST_SET_BYTES: usize = 16 * 1024;

/// Verify a release manifest against the public trust set configured at build time.
pub(crate) fn verify_release_manifest(manifest: &[u8], envelope: &[u8]) -> anyhow::Result<String> {
    let keys = embedded_trust_keys()?;
    verify_release_manifest_with_keys(manifest, envelope, &keys)
}

pub(crate) fn verify_release_manifest_with_keys(
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
    if !valid_key_id(key_id) {
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
    trust_keys_from_inputs(
        option_env!("AETHER_TUNNEL_RELEASE_TRUST_KEYS"),
        option_env!("AETHER_TUNNEL_RELEASE_KEY_ID"),
        option_env!("AETHER_TUNNEL_RELEASE_PUBLIC_KEY"),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedPublicKey {
    key_id: String,
    public_key: String,
}

fn valid_key_id(key_id: &str) -> bool {
    !key_id.is_empty()
        && key_id.len() <= MAX_KEY_ID_BYTES
        && key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn parse_public_key(public_key: &str) -> anyhow::Result<VerifyingKey> {
    let bytes = BASE64
        .decode(public_key)
        .map_err(|_| anyhow::anyhow!("tunnel release verifier has an invalid trusted key"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("tunnel release verifier has an invalid trusted key"))?;
    let key = VerifyingKey::from_bytes(&bytes)
        .map_err(|_| anyhow::anyhow!("tunnel release verifier has an invalid trusted key"))?;
    if key.is_weak() {
        anyhow::bail!("tunnel release verifier has a weak trusted key");
    }
    Ok(key)
}

/// Shared with the release build checker; inputs are public data only.
/// A nonempty trust set is authoritative. Legacy inputs must agree with it,
/// so stale single-key configuration cannot silently retain a retired key.
pub(crate) fn trust_keys_from_inputs(
    trust_set: Option<&str>,
    key_id: Option<&str>,
    public_key: Option<&str>,
) -> anyhow::Result<BTreeMap<String, VerifyingKey>> {
    // GitHub exposes an unset repository variable as an empty string.
    let trust_set = trust_set.filter(|value| !value.is_empty());
    let key_id = key_id.filter(|value| !value.is_empty());
    let public_key = public_key.filter(|value| !value.is_empty());
    if key_id.is_some_and(|value| !valid_key_id(value)) {
        anyhow::bail!("tunnel release verifier has an invalid trusted key_id");
    }
    let mut keys = BTreeMap::new();
    if let Some(trust_set) = trust_set {
        if trust_set.len() > MAX_TRUST_SET_BYTES {
            anyhow::bail!("tunnel release verifier trust set is too large");
        }
        let entries: Vec<TrustedPublicKey> = serde_json::from_str(trust_set)
            .map_err(|_| anyhow::anyhow!("tunnel release verifier has a malformed trust set"))?;
        if entries.is_empty() || entries.len() > MAX_TRUST_KEYS {
            anyhow::bail!("tunnel release verifier trust set has an invalid key count");
        }
        for entry in entries {
            if !valid_key_id(&entry.key_id) {
                anyhow::bail!("tunnel release verifier has an invalid trusted key_id");
            }
            let key = parse_public_key(&entry.public_key)?;
            if keys.values().any(|existing| existing == &key) {
                anyhow::bail!("tunnel release verifier trust set has a duplicate public key");
            }
            if keys.insert(entry.key_id, key).is_some() {
                anyhow::bail!("tunnel release verifier trust set has a duplicate key_id");
            }
        }
        if let Some(key_id) = key_id {
            let trusted = keys.get(key_id).ok_or_else(|| {
                anyhow::anyhow!("tunnel release signing key_id is not in the trust set")
            })?;
            if let Some(public_key) = public_key {
                if &parse_public_key(public_key)? != trusted {
                    anyhow::bail!("tunnel release legacy public key conflicts with the trust set");
                }
            }
        } else if public_key.is_some() {
            anyhow::bail!("tunnel release legacy public key is missing key_id");
        }
    } else {
        let (Some(key_id), Some(public_key)) = (key_id, public_key) else {
            anyhow::bail!("tunnel release verifier has no trusted key");
        };
        keys.insert(key_id.to_string(), parse_public_key(public_key)?);
    }
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

    fn public_key(seed: u8) -> String {
        BASE64.encode(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes(),
        )
    }

    fn trust_set(entries: &[(&str, u8)]) -> String {
        serde_json::to_string(
            &entries
                .iter()
                .map(|(id, seed)| {
                    serde_json::json!({ "key_id": id, "public_key": public_key(*seed) })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn signed_envelope(manifest: &[u8], key_id: &str, seed: u8) -> Vec<u8> {
        let signature = SigningKey::from_bytes(&[seed; 32]).sign(manifest);
        format!(
            "version=1\nkey_id={key_id}\nsignature={}\n",
            BASE64.encode(signature.to_bytes())
        )
        .into_bytes()
    }

    #[test]
    fn rotation_overlap_switch_and_retirement_verify_real_signatures() {
        let manifest = b"abc123  aether-tunnel-linux-amd64.tar.gz\n";
        let old = signed_envelope(manifest, "old", 7);
        let new = signed_envelope(manifest, "new", 8);
        let initial = trust_keys_from_inputs(None, Some("old"), Some(&public_key(7))).unwrap();
        let overlap = trust_set(&[("old", 7), ("new", 8)]);
        // Both the bridge release (old signer) and the switched release embed both keys.
        for signer in ["old", "new"] {
            let keys = trust_keys_from_inputs(Some(&overlap), Some(signer), None).unwrap();
            assert_eq!(
                verify_release_manifest_with_keys(manifest, &old, &keys).unwrap(),
                "old"
            );
            assert_eq!(
                verify_release_manifest_with_keys(manifest, &new, &keys).unwrap(),
                "new"
            );
            let mislabeled = signed_envelope(manifest, "new", 7);
            assert!(verify_release_manifest_with_keys(manifest, &mislabeled, &keys).is_err());
            let unknown = signed_envelope(manifest, "unknown", 8);
            assert!(verify_release_manifest_with_keys(manifest, &unknown, &keys).is_err());
            assert!(verify_release_manifest_with_keys(b"tampered", &new, &keys).is_err());
        }
        assert!(verify_release_manifest_with_keys(manifest, &old, &initial).is_ok());
        assert!(verify_release_manifest_with_keys(manifest, &new, &initial).is_err());
        let retired = trust_keys_from_inputs(Some(&trust_set(&[("new", 8)])), None, None).unwrap();
        assert!(verify_release_manifest_with_keys(manifest, &new, &retired).is_ok());
        assert!(verify_release_manifest_with_keys(manifest, &old, &retired).is_err());
    }

    #[test]
    fn legacy_configuration_and_empty_github_variables_remain_compatible() {
        let public = public_key(7);
        for value in [None, Some("")] {
            let keys = trust_keys_from_inputs(value, Some("old"), Some(&public)).unwrap();
            assert_eq!(keys.len(), 1);
        }
        let overlap = trust_set(&[("old", 7), ("new", 8)]);
        assert!(trust_keys_from_inputs(Some(&overlap), Some("old"), Some(&public)).is_ok());
        assert!(trust_keys_from_inputs(Some(&overlap), Some(""), Some("")).is_ok());
    }

    #[test]
    fn missing_conflicting_and_unknown_build_inputs_fail_closed() {
        let overlap = trust_set(&[("old", 7), ("new", 8)]);
        for inputs in [
            (None, None, None),
            (Some(""), Some(""), Some("")),
            (None, Some("old"), None),
            (None, None, Some(public_key(7).as_str())),
            (Some(overlap.as_str()), Some("unknown"), None),
            (Some(overlap.as_str()), None, Some(public_key(7).as_str())),
            (
                Some(overlap.as_str()),
                Some("old"),
                Some(public_key(8).as_str()),
            ),
            // A stale legacy value must not reintroduce a retired key.
            (
                Some(trust_set(&[("new", 8)]).as_str()),
                Some("old"),
                Some(public_key(7).as_str()),
            ),
        ] {
            assert!(trust_keys_from_inputs(inputs.0, inputs.1, inputs.2).is_err());
        }
    }

    #[test]
    fn duplicate_malformed_and_unknown_trust_entries_fail_closed() {
        let public = public_key(7);
        for input in [
            "null".to_string(),
            "{}".to_string(),
            "[]".to_string(),
            " ".to_string(),
            "[".to_string(),
            "[{}]".to_string(),
            "[null]".to_string(),
            format!(r#"[{{"key_id":"old","public_key":"{public}","private_key":"forbidden"}}]"#),
            format!(r#"[{{"key_id":"old","key_id":"new","public_key":"{public}"}}]"#),
            format!(r#"[{{"key_id":"old","public_key":"{public}","public_key":"{public}"}}]"#),
            trust_set(&[("old", 7), ("old", 8)]),
            trust_set(&[("old", 7), ("new", 7)]),
            format!("{} trailing", trust_set(&[("old", 7)])),
            " ".repeat(MAX_TRUST_SET_BYTES + 1),
        ] {
            assert!(trust_keys_from_inputs(Some(&input), None, None).is_err());
        }
        let entries = (0..=MAX_TRUST_KEYS)
            .map(|index| serde_json::json!({ "key_id": format!("key-{index}"), "public_key": public_key(index as u8) }))
            .collect::<Vec<_>>();
        assert!(trust_keys_from_inputs(
            Some(&serde_json::to_string(&entries).unwrap()),
            None,
            None
        )
        .is_err());
    }

    #[test]
    fn invalid_ids_and_public_keys_are_rejected_for_both_input_formats() {
        for id in [
            "",
            " leading",
            "trailing ",
            "a=b",
            "a\nb",
            "非ascii",
            &"a".repeat(129),
        ] {
            assert!(trust_keys_from_inputs(None, Some(id), Some(&public_key(7))).is_err());
            assert!(trust_keys_from_inputs(Some(&trust_set(&[(id, 7)])), None, None).is_err());
        }
        for public in [
            "".to_string(),
            "%%%".to_string(),
            "AA==".to_string(),
            BASE64.encode([0u8; 32]),
            BASE64.encode([7u8; 33]),
        ] {
            assert!(trust_keys_from_inputs(None, Some("old"), Some(&public)).is_err());
            let set = serde_json::json!([{ "key_id": "old", "public_key": public }]).to_string();
            assert!(trust_keys_from_inputs(Some(&set), None, None).is_err());
        }
    }
}
