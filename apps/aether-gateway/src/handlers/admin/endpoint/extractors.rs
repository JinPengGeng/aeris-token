pub(super) fn admin_health_key_id(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/endpoints/health/key/")?;
    let normalized = raw.trim().trim_matches('/');
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub(super) fn admin_recover_key_id(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/endpoints/health/keys/")?;
    let normalized = raw.trim().trim_matches('/');
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub(super) fn admin_repair_half_open_isolation_key_id(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/endpoints/health/keys/")?;
    let key_id = raw.strip_suffix("/half-open-isolation")?;
    let normalized = key_id.trim().trim_matches('/');
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub(super) fn admin_rpm_key_id(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/endpoints/rpm/key/")?;
    let normalized = raw.trim().trim_matches('/');
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}
