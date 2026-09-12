//! Key rotation contract for tunnel handshake signatures.
//!
//! A rotation keeps the previous key valid until its explicit expiry.  The
//! verifier receives the key id from the wire and never falls back to another
//! key, which makes rollback and revocation deterministic.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelSigningKey {
    pub key_id: String,
    pub key: String,
    /// Inclusive start of validity, in unix seconds.
    pub not_before: u64,
    /// Exclusive end of validity, in unix seconds. `None` means no expiry.
    pub expires_at: Option<u64>,
}

impl TunnelSigningKey {
    pub fn valid_at(&self, now: u64) -> bool {
        now >= self.not_before && self.expires_at.is_none_or(|end| now < end)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TunnelSigningKeySet {
    keys: BTreeMap<String, TunnelSigningKey>,
    active_signing_key_id: Option<String>,
}

impl TunnelSigningKeySet {
    pub fn new(
        keys: impl IntoIterator<Item = TunnelSigningKey>,
        active_signing_key_id: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let mut set = Self::default();
        for key in keys {
            if key.key_id.is_empty() || key.expires_at.is_some_and(|end| end <= key.not_before) {
                return Err("invalid tunnel signing key interval");
            }
            set.keys.insert(key.key_id.clone(), key);
        }
        let active = active_signing_key_id.into();
        if !set.keys.contains_key(&active) {
            return Err("active tunnel signing key is missing");
        }
        set.active_signing_key_id = Some(active);
        Ok(set)
    }

    pub fn signing_key(&self, now: u64) -> Option<&TunnelSigningKey> {
        self.active_signing_key_id
            .as_ref()
            .and_then(|id| self.keys.get(id))
            .filter(|key| key.valid_at(now))
    }

    pub fn verification_key(&self, key_id: &str, now: u64) -> Option<&TunnelSigningKey> {
        self.keys.get(key_id).filter(|key| key.valid_at(now))
    }

    /// Select a valid key for newly-created handshakes (used for rollback).
    pub fn set_active_signing_key(&mut self, key_id: &str, now: u64) -> Result<(), &'static str> {
        if self.verification_key(key_id, now).is_none() {
            return Err("active tunnel signing key is not valid");
        }
        self.active_signing_key_id = Some(key_id.to_owned());
        Ok(())
    }

    pub fn revoke(&mut self, key_id: &str) -> bool {
        self.keys.remove(key_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str, start: u64, end: Option<u64>) -> TunnelSigningKey {
        TunnelSigningKey {
            key_id: id.into(),
            key: "secret".into(),
            not_before: start,
            expires_at: end,
        }
    }

    #[test]
    fn overlap_accepts_old_and_new_until_old_expiry() {
        let set = TunnelSigningKeySet::new([key("old", 0, Some(20)), key("new", 10, None)], "new")
            .unwrap();
        assert!(set.verification_key("old", 10).is_some());
        assert!(set.verification_key("new", 10).is_some());
        assert!(set.verification_key("old", 20).is_none());
        assert_eq!(set.signing_key(10).unwrap().key_id, "new");
    }

    #[test]
    fn rollback_requires_explicit_active_key_and_respects_window() {
        let mut set =
            TunnelSigningKeySet::new([key("old", 0, None), key("new", 10, None)], "new").unwrap();
        assert!(set.signing_key(5).is_none());
        assert!(set.revoke("new"));
        assert!(set.signing_key(11).is_none());
        assert!(set.verification_key("old", 11).is_some());
        set.set_active_signing_key("old", 11).unwrap();
        assert_eq!(set.signing_key(11).unwrap().key_id, "old");
    }
}
