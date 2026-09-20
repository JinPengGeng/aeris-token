use super::{
    extract_admin_provider_oauth_batch_import_entry, parse_error_entry,
    AdminProviderOAuthBatchImportEntry,
};
use serde_json::json;

pub(super) fn parse_sub2api_export_accounts(
    provider_type: &str,
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<Vec<AdminProviderOAuthBatchImportEntry>> {
    if !provider_type.trim().eq_ignore_ascii_case("codex") {
        return None;
    }
    let is_sub2api_export = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("sub2api-data"));
    if !is_sub2api_export {
        return None;
    }

    let Some(accounts) = object.get("accounts").and_then(serde_json::Value::as_array) else {
        return Some(vec![parse_error_entry(
            "sub2api 导出缺少 accounts 数组".to_string(),
        )]);
    };

    let mut entries = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        let Some(account) = account.as_object() else {
            entries.push(parse_error_entry(format!(
                "sub2api 第 {} 个账号必须是 JSON 对象",
                index + 1
            )));
            continue;
        };
        if account
            .get("platform")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|platform| !platform.trim().eq_ignore_ascii_case("openai"))
        {
            continue;
        }

        let Some(mut credentials) = account
            .get("credentials")
            .and_then(serde_json::Value::as_object)
            .cloned()
        else {
            entries.push(parse_error_entry(format!(
                "sub2api 第 {} 个账号缺少 credentials 对象",
                index + 1
            )));
            continue;
        };
        if let Some(name) = account
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            credentials
                .entry("account_name".to_string())
                .or_insert_with(|| json!(name));
        }
        if let Some(extra) = account.get("extra").and_then(serde_json::Value::as_object) {
            for key in [
                "account_id",
                "chatgpt_account_id",
                "chatgpt_user_id",
                "chatgpt_account_is_fedramp",
                "email",
                "plan_type",
                "workspace_id",
            ] {
                if let Some(value) = extra.get(key).cloned() {
                    credentials.entry(key.to_string()).or_insert(value);
                }
            }
        }

        let credentials = serde_json::Value::Object(credentials);
        match extract_admin_provider_oauth_batch_import_entry(provider_type, &credentials) {
            Some(entry) => entries.push(entry),
            None => entries.push(parse_error_entry(format!(
                "sub2api 第 {} 个账号没有可导入的凭据",
                index + 1
            ))),
        }
    }

    if entries.is_empty() {
        entries.push(parse_error_entry(
            "sub2api 导出中没有可导入的 OpenAI 账号".to_string(),
        ));
    }
    Some(entries)
}
