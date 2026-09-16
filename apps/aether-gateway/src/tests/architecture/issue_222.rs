use super::read_workspace_file;

#[test]
fn issue_222_extension_surfaces_are_explicit_and_documented() {
    let provider_types =
        read_workspace_file("crates/aether-provider/transport/src/provider_types.rs");
    let provider_pool = read_workspace_file("crates/aether-provider/pool/src/service.rs");
    let provider_pool_modules =
        read_workspace_file("crates/aether-provider/pool/src/providers/mod.rs");
    let provider_oauth = read_workspace_file("crates/aether-oauth/src/provider/service.rs");
    let architecture = read_workspace_file("docs/architecture/adr-0051-extension-surfaces.md");

    // Fixed provider metadata has one source of truth. Keep this list derived from
    // the source so adding a template also requires an architecture-record update.
    let fixed_provider_types = provider_types
        .lines()
        .filter_map(|line| line.trim().strip_prefix("provider_type: \""))
        .filter_map(|value| value.split_once('"').map(|(provider, _)| provider))
        .collect::<Vec<_>>();
    assert!(!fixed_provider_types.is_empty());
    for provider_type in fixed_provider_types {
        assert!(
            architecture.contains(&format!("| `{provider_type}` |")),
            "fixed provider {provider_type} must be listed in ADR-0051"
        );
    }

    for marker in [
        "pub fn fixed_provider_template(",
        "pub fn provider_runtime_policy(",
        "pub fn with_builtin_adapters(",
        "pub fn with_adapter(",
    ] {
        assert!(
            provider_types.contains(marker)
                || provider_pool.contains(marker)
                || provider_oauth.contains(marker),
            "provider extension marker missing: {marker}"
        );
    }
    for marker in [
        "pub mod grok;",
        "pub mod xai;",
        "GrokProviderPoolAdapter",
        "XaiProviderPoolAdapter",
    ] {
        assert!(
            provider_pool_modules.contains(marker),
            "provider pool registration marker missing: {marker}"
        );
    }
    for marker in [
        "ProviderOAuthService",
        "with_builtin_adapters()",
        "GenericProviderOAuthAdapter::for_provider_type",
    ] {
        assert!(
            provider_oauth.contains(marker),
            "OAuth registry marker missing: {marker}"
        );
    }
}

#[test]
fn issue_222_data_layer_contract_keeps_postgres_boundary_and_migration_sources_explicit() {
    let database = read_workspace_file("crates/aether-data/contracts/src/database.rs");
    let runtime_manifest = read_workspace_file("crates/aether-data/runtime/Cargo.toml");
    let runtime_readme = read_workspace_file("crates/aether-data/runtime/README.md");
    let architecture = read_workspace_file("docs/architecture/adr-0051-extension-surfaces.md");

    assert!(database.contains("pub enum DatabaseDriver {\n    Postgres,"));
    assert!(!database.contains("Mysql") && !database.contains("Sqlite"));
    assert!(runtime_manifest.contains("default = [\"postgres\"]"));
    assert!(runtime_manifest.contains("all-drivers = [\"postgres\"]"));
    for marker in [
        "schema/logical",
        "schema/generated",
        "adapters/postgres/migrations",
        "compose_schema.sh check",
    ] {
        assert!(
            runtime_readme.contains(marker),
            "data-layer source marker missing: {marker}"
        );
        assert!(
            architecture.contains(marker),
            "ADR-0051 must preserve source marker: {marker}"
        );
    }
}
