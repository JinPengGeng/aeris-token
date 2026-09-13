//! Release build input validation and offline verification using production code.

#[path = "../../../../apps/aether-tunnel/src/setup/provenance.rs"]
mod provenance;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [command, manifest, envelope] = args.as_slice() {
        if command == "verify-embedded" {
            provenance::verify_release_manifest(
                &std::fs::read(manifest)?,
                &std::fs::read(envelope)?,
            )?;
            return Ok(());
        }
    }

    let trust_set = std::env::var("AETHER_TUNNEL_RELEASE_TRUST_KEYS").ok();
    let key_id = std::env::var("AETHER_TUNNEL_RELEASE_KEY_ID").ok();
    let public_key = std::env::var("AETHER_TUNNEL_RELEASE_PUBLIC_KEY").ok();
    let unconfigured = [&trust_set, &key_id, &public_key]
        .iter()
        .all(|input| input.as_deref().unwrap_or("").is_empty());
    if args == ["check", "--allow-unconfigured"] && unconfigured {
        // A local/manual build may omit trust entirely; its updater fails closed.
        return Ok(());
    }
    let keys = provenance::trust_keys_from_inputs(
        trust_set.as_deref(),
        key_id.as_deref(),
        public_key.as_deref(),
    )?;
    let signing_id = key_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("release signing key_id is required"))?;
    match args.as_slice() {
        [command] if command == "check" => Ok(()),
        [command, flag] if command == "check" && flag == "--allow-unconfigured" => Ok(()),
        [command, manifest, envelope] if command == "verify" => {
            let verified = provenance::verify_release_manifest_with_keys(
                &std::fs::read(manifest)?,
                &std::fs::read(envelope)?,
                &keys,
            )?;
            if verified != signing_id {
                anyhow::bail!("release signature key_id does not match the configured signer");
            }
            Ok(())
        }
        _ => anyhow::bail!("expected check [--allow-unconfigured] or verify <manifest> <envelope>"),
    }
}
