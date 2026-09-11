use serde_json::{json, Map, Value};

pub const DEFAULT_SUB2API_PROGRESS_ENDPOINT: &str = "/api/v1/subscriptions/progress";
pub const DEFAULT_REMOTE_QUOTA_FETCH_INTERVAL_SECS: u64 = 300;
pub const MIN_REMOTE_QUOTA_FETCH_INTERVAL_SECS: u64 = 60;
const DAY_SECONDS: u64 = 24 * 60 * 60;
const MAX_FUTURE_WINDOW_START_SKEW_SECONDS: u64 = 5 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sub2ApiRemoteQuotaConfig {
    pub group_id: String,
    pub progress_endpoint: String,
    pub fetch_interval_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sub2ApiQuotaWindowKind {
    Daily,
    Weekly,
    Monthly,
}

impl Sub2ApiQuotaWindowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }

    fn interval_days(self) -> u64 {
        match self {
            Self::Daily => 1,
            Self::Weekly => 7,
            Self::Monthly => 30,
        }
    }

    pub fn display_name_zh(self) -> &'static str {
        match self {
            Self::Daily => "日",
            Self::Weekly => "周",
            Self::Monthly => "月",
        }
    }

    fn kinds() -> [Self; 3] {
        [Self::Daily, Self::Weekly, Self::Monthly]
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Sub2ApiRemoteQuotaWindow {
    pub kind: Sub2ApiQuotaWindowKind,
    pub limit_usd: f64,
    pub used_usd: f64,
    pub window_start_unix_secs: u64,
    pub resets_at_unix_secs: u64,
}

impl Sub2ApiRemoteQuotaWindow {
    pub fn is_exhausted_at(&self, now_unix_secs: u64) -> bool {
        self.used_usd >= self.limit_usd && now_unix_secs < self.resets_at_unix_secs
    }

    pub fn used_ratio(&self) -> f64 {
        if self.limit_usd <= 0.0 {
            return 0.0;
        }
        (self.used_usd / self.limit_usd).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sub2ApiRemoteQuotaObservation {
    pub subscription_id: String,
    pub group_id: String,
    pub group_name: String,
    pub status: String,
    pub expires_at_unix_secs: Option<u64>,
    pub windows: Vec<Sub2ApiRemoteQuotaWindow>,
}

impl Sub2ApiRemoteQuotaObservation {
    pub fn exhausted_windows_at(
        &self,
        now_unix_secs: u64,
    ) -> impl Iterator<Item = &Sub2ApiRemoteQuotaWindow> {
        let now = now_unix_secs;
        self.windows
            .iter()
            .filter(move |window| window.is_exhausted_at(now))
    }

    pub fn max_usage_ratio(&self) -> Option<f64> {
        self.windows
            .iter()
            .map(Sub2ApiRemoteQuotaWindow::used_ratio)
            .max_by(f64::total_cmp)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sub2ApiRemoteQuotaState {
    /// 配置的分组没有有效订阅：无订阅记录、status 非 active 或已过期。
    SubscriptionInvalid {
        group_id: String,
        subscription_status: Option<String>,
        expires_at_unix_secs: Option<u64>,
    },
    Active(Sub2ApiRemoteQuotaObservation),
}

#[derive(Debug, Clone)]
struct SummarySubscription {
    id: String,
    group_id: String,
    group_name: String,
    status: String,
    daily_limit_usd: f64,
    weekly_limit_usd: f64,
    monthly_limit_usd: f64,
    expires_at_unix_secs: Option<u64>,
}

impl SummarySubscription {
    fn is_active_at(&self, now_unix_secs: u64) -> bool {
        self.status.eq_ignore_ascii_case("active")
            && self
                .expires_at_unix_secs
                .is_none_or(|expires_at| expires_at > now_unix_secs)
    }

    fn limited_windows(&self) -> impl Iterator<Item = (Sub2ApiQuotaWindowKind, f64)> {
        [
            (Sub2ApiQuotaWindowKind::Daily, self.daily_limit_usd),
            (Sub2ApiQuotaWindowKind::Weekly, self.weekly_limit_usd),
            (Sub2ApiQuotaWindowKind::Monthly, self.monthly_limit_usd),
        ]
        .into_iter()
        .filter(|(_, limit_usd)| *limit_usd > 0.0)
    }
}

#[derive(Debug, Clone)]
struct ProgressSubscription {
    subscription_id: String,
    group_id: String,
    daily: Option<Sub2ApiRemoteQuotaWindow>,
    weekly: Option<Sub2ApiRemoteQuotaWindow>,
    monthly: Option<Sub2ApiRemoteQuotaWindow>,
    expires_at_unix_secs: Option<u64>,
}

impl ProgressSubscription {
    fn window(&self, kind: Sub2ApiQuotaWindowKind) -> Option<&Sub2ApiRemoteQuotaWindow> {
        match kind {
            Sub2ApiQuotaWindowKind::Daily => self.daily.as_ref(),
            Sub2ApiQuotaWindowKind::Weekly => self.weekly.as_ref(),
            Sub2ApiQuotaWindowKind::Monthly => self.monthly.as_ref(),
        }
    }
}

pub fn parse_sub2api_remote_quota_config(
    provider_ops_config: &Map<String, Value>,
) -> Result<Option<Sub2ApiRemoteQuotaConfig>, String> {
    let Some(remote_quota) = provider_ops_config.get("remote_quota") else {
        return Ok(None);
    };
    let remote_quota = remote_quota
        .as_object()
        .ok_or_else(|| "remote_quota 必须是 JSON 对象".to_string())?;
    if remote_quota.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Ok(None);
    }

    let group_id = canonical_json_id(remote_quota.get("group_id"))
        .ok_or_else(|| "remote_quota.group_id 不能为空".to_string())?;
    let progress_endpoint = remote_quota
        .get("progress_endpoint")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_SUB2API_PROGRESS_ENDPOINT);
    validate_sub2api_same_origin_endpoint(progress_endpoint)?;
    let fetch_interval_seconds = match remote_quota.get("fetch_interval_seconds") {
        None | Some(Value::Null) => DEFAULT_REMOTE_QUOTA_FETCH_INTERVAL_SECS,
        Some(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| "remote_quota.fetch_interval_seconds 必须是正整数".to_string())?,
    };

    Ok(Some(Sub2ApiRemoteQuotaConfig {
        group_id,
        progress_endpoint: progress_endpoint.to_string(),
        fetch_interval_seconds: fetch_interval_seconds.max(MIN_REMOTE_QUOTA_FETCH_INTERVAL_SECS),
    }))
}

pub fn validate_sub2api_same_origin_endpoint(endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if !endpoint.starts_with('/')
        || endpoint.starts_with("//")
        || endpoint.contains('#')
        || endpoint.contains('\\')
    {
        return Err("Sub2API 配额端点必须是同源、以 / 开头且不含 fragment 的相对路径".to_string());
    }
    Ok(())
}

pub fn parse_sub2api_remote_quota(
    summary_json: &Value,
    progress_json: Option<&Value>,
    expected_group_id: &str,
) -> Result<Sub2ApiRemoteQuotaState, String> {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    parse_sub2api_remote_quota_at(
        summary_json,
        progress_json,
        expected_group_id,
        now_unix_secs,
    )
}

pub fn parse_sub2api_remote_quota_at(
    summary_json: &Value,
    progress_json: Option<&Value>,
    expected_group_id: &str,
    now_unix_secs: u64,
) -> Result<Sub2ApiRemoteQuotaState, String> {
    let expected_group_id = expected_group_id.trim();
    if expected_group_id.is_empty() {
        return Err("remote_quota.group_id 不能为空".to_string());
    }

    let subscriptions = parse_summary_subscriptions(summary_json)?;
    let matching = subscriptions
        .iter()
        .filter(|subscription| subscription.group_id == expected_group_id)
        .collect::<Vec<_>>();
    let active = matching
        .iter()
        .filter(|subscription| subscription.is_active_at(now_unix_secs))
        .collect::<Vec<_>>();
    if active.len() > 1 {
        return Err(format!(
            "Sub2API Group {expected_group_id} 存在多个活跃套餐，无法确定唯一额度"
        ));
    }
    let Some(subscription) = active.first().copied() else {
        let representative = matching
            .iter()
            .max_by_key(|subscription| subscription.expires_at_unix_secs.unwrap_or(0));
        return Ok(Sub2ApiRemoteQuotaState::SubscriptionInvalid {
            group_id: expected_group_id.to_string(),
            subscription_status: representative.map(|subscription| subscription.status.clone()),
            expires_at_unix_secs: representative.and_then(|subscription| {
                subscription
                    .expires_at_unix_secs
                    .filter(|expires_at| *expires_at > now_unix_secs)
            }),
        });
    };

    let limited_windows = subscription.limited_windows().collect::<Vec<_>>();
    if limited_windows.is_empty() {
        return Ok(Sub2ApiRemoteQuotaState::Active(
            Sub2ApiRemoteQuotaObservation {
                subscription_id: subscription.id.clone(),
                group_id: subscription.group_id.clone(),
                group_name: subscription.group_name.clone(),
                status: subscription.status.clone(),
                expires_at_unix_secs: subscription.expires_at_unix_secs,
                windows: Vec::new(),
            },
        ));
    }

    let progress_json = progress_json.ok_or_else(|| {
        format!(
            "Sub2API Group {} 的有限额度同步需要 subscriptions/progress 响应",
            subscription.group_id
        )
    })?;
    let progresses = parse_progress_subscriptions(progress_json, now_unix_secs)?;
    let matching_progress = progresses
        .iter()
        .filter(|progress| {
            progress.subscription_id == subscription.id
                && progress.group_id == subscription.group_id
        })
        .collect::<Vec<_>>();
    if matching_progress.len() > 1 {
        return Err(format!(
            "Sub2API Group {} 存在多个匹配的 progress 记录，无法确定唯一额度窗口",
            subscription.group_id
        ));
    }
    let progress = matching_progress.first().copied().ok_or_else(|| {
        format!(
            "Sub2API Group {} 的有限额度 progress 数据缺失",
            subscription.group_id
        )
    })?;

    let mut windows = Vec::with_capacity(limited_windows.len());
    for (kind, summary_limit_usd) in limited_windows {
        let window = progress.window(kind).ok_or_else(|| {
            format!(
                "Sub2API Group {} 已配置{}额度，但 progress.{} 没有可用的当前额度窗口",
                subscription.group_id,
                kind.display_name_zh(),
                kind.as_str(),
            )
        })?;
        let limit_tolerance = summary_limit_usd.abs().max(1.0) * 1e-9;
        if (window.limit_usd - summary_limit_usd).abs() > limit_tolerance {
            return Err(format!(
                "Sub2API Group {} 的{}额度在 summary 与 progress 中不一致",
                subscription.group_id,
                kind.as_str(),
            ));
        }
        windows.push(window.clone());
    }

    Ok(Sub2ApiRemoteQuotaState::Active(
        Sub2ApiRemoteQuotaObservation {
            subscription_id: subscription.id.clone(),
            group_id: subscription.group_id.clone(),
            group_name: subscription.group_name.clone(),
            status: subscription.status.clone(),
            expires_at_unix_secs: subscription
                .expires_at_unix_secs
                .or(progress.expires_at_unix_secs),
            windows,
        },
    ))
}

fn parse_summary_subscriptions(payload: &Value) -> Result<Vec<SummarySubscription>, String> {
    let data = successful_envelope_data(payload, "Sub2API 套餐摘要")?;
    let items = if let Some(items) = data.as_array() {
        items
    } else {
        data.as_object()
            .and_then(|data| data.get("subscriptions"))
            .and_then(Value::as_array)
            .ok_or_else(|| "Sub2API 套餐摘要缺少 subscriptions 数组".to_string())?
    };
    let parsed = items
        .iter()
        .filter_map(|value| parse_summary_subscription(value).ok())
        .collect::<Vec<_>>();
    if !items.is_empty() && parsed.is_empty() {
        return Err("Sub2API 套餐摘要没有可用条目".to_string());
    }
    Ok(parsed)
}

fn parse_summary_subscription(value: &Value) -> Result<SummarySubscription, String> {
    let item = value
        .as_object()
        .ok_or_else(|| "Sub2API 套餐元素必须是 JSON 对象".to_string())?;
    let id = canonical_json_id(item.get("id")).ok_or_else(|| "Sub2API 套餐缺少 id".to_string())?;
    let group = item.get("group").and_then(Value::as_object);
    let group_id = canonical_json_id(item.get("group_id"))
        .or_else(|| group.and_then(|group| canonical_json_id(group.get("id"))))
        .ok_or_else(|| format!("Sub2API 套餐 {id} 缺少 group_id"))?;
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("Sub2API 套餐 {id} 缺少 status"))?;
    let group_name = item
        .get("group_name")
        .and_then(Value::as_str)
        .or_else(|| {
            group
                .and_then(|group| group.get("name"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .trim()
        .to_string();
    let daily_limit_usd = summary_limit_usd(item, group, "daily")?;
    let weekly_limit_usd = summary_limit_usd(item, group, "weekly")?;
    let monthly_limit_usd = summary_limit_usd(item, group, "monthly")?;
    for window in ["daily", "weekly", "monthly"] {
        summary_used_usd(item, window)?;
    }

    Ok(SummarySubscription {
        id,
        group_id,
        group_name,
        status,
        daily_limit_usd,
        weekly_limit_usd,
        monthly_limit_usd,
        expires_at_unix_secs: optional_timestamp_unix_secs(
            item.get("expires_at_unix_secs")
                .or_else(|| item.get("expires_at")),
            "expires_at",
        )?,
    })
}

fn summary_limit_usd(
    item: &Map<String, Value>,
    group: Option<&Map<String, Value>>,
    window: &str,
) -> Result<f64, String> {
    let field = format!("{window}_limit_usd");
    optional_non_negative_f64(
        item.get(&field)
            .filter(|value| !value.is_null())
            .or_else(|| {
                group
                    .and_then(|group| group.get(&field))
                    .filter(|value| !value.is_null())
            }),
        &field,
    )
    .map(|value| value.unwrap_or(0.0))
}

fn summary_used_usd(item: &Map<String, Value>, window: &str) -> Result<f64, String> {
    let used_field = format!("{window}_used_usd");
    let usage_field = format!("{window}_usage_usd");
    optional_non_negative_f64(
        item.get(&used_field)
            .filter(|value| !value.is_null())
            .or_else(|| item.get(&usage_field).filter(|value| !value.is_null())),
        &used_field,
    )
    .map(|value| value.unwrap_or(0.0))
}

fn parse_progress_subscriptions(
    payload: &Value,
    now_unix_secs: u64,
) -> Result<Vec<ProgressSubscription>, String> {
    let data = successful_envelope_data(payload, "Sub2API 套餐进度")?;
    let items: Vec<&Value> = if let Some(items) = data.as_array() {
        items.iter().collect()
    } else if data.is_object() {
        std::iter::once(data).collect()
    } else {
        return Err("Sub2API 套餐进度 data 必须是数组或对象".to_string());
    };
    let parsed = items
        .iter()
        .filter_map(|value| parse_progress_subscription(value, now_unix_secs).ok())
        .collect::<Vec<_>>();
    if !items.is_empty() && parsed.is_empty() {
        return Err("Sub2API 套餐进度没有可用条目".to_string());
    }
    Ok(parsed)
}

fn parse_progress_subscription(
    value: &Value,
    now_unix_secs: u64,
) -> Result<ProgressSubscription, String> {
    let item = value
        .as_object()
        .ok_or_else(|| "Sub2API 套餐进度元素必须是 JSON 对象".to_string())?;
    // 同时接受扁平契约（subscription_id/group_id + daily/weekly/monthly 顶层字段）
    // 与上游嵌套契约（{subscription: {...}, progress: {...}}）。
    let (identity, windows_source) = match item.get("progress").and_then(Value::as_object) {
        Some(progress) => (
            item.get("subscription")
                .and_then(Value::as_object)
                .unwrap_or(item),
            progress,
        ),
        None => (item, item),
    };
    let subscription_id = canonical_json_id(identity.get("id"))
        .or_else(|| canonical_json_id(identity.get("subscription_id")))
        .or_else(|| canonical_json_id(windows_source.get("id")))
        .or_else(|| canonical_json_id(windows_source.get("subscription_id")))
        .ok_or_else(|| "Sub2API 套餐进度缺少 subscription_id".to_string())?;
    let group_id = canonical_json_id(identity.get("group_id"))
        .or_else(|| {
            identity
                .get("group")
                .and_then(Value::as_object)
                .and_then(|group| canonical_json_id(group.get("id")))
        })
        .or_else(|| canonical_json_id(windows_source.get("group_id")))
        .ok_or_else(|| format!("Sub2API 套餐进度 {subscription_id} 缺少 group_id"))?;

    let mut daily = None;
    let mut weekly = None;
    let mut monthly = None;
    for kind in Sub2ApiQuotaWindowKind::kinds() {
        let window = parse_progress_window(windows_source, kind, now_unix_secs)?;
        match kind {
            Sub2ApiQuotaWindowKind::Daily => daily = window,
            Sub2ApiQuotaWindowKind::Weekly => weekly = window,
            Sub2ApiQuotaWindowKind::Monthly => monthly = window,
        }
    }

    Ok(ProgressSubscription {
        subscription_id,
        group_id,
        daily,
        weekly,
        monthly,
        expires_at_unix_secs: optional_timestamp_unix_secs(
            windows_source
                .get("expires_at_unix_secs")
                .or_else(|| windows_source.get("expires_at"))
                .or_else(|| identity.get("expires_at_unix_secs"))
                .or_else(|| identity.get("expires_at")),
            "expires_at",
        )?,
    })
}

fn parse_progress_window(
    progress: &Map<String, Value>,
    kind: Sub2ApiQuotaWindowKind,
    now_unix_secs: u64,
) -> Result<Option<Sub2ApiRemoteQuotaWindow>, String> {
    let Some(value) = progress.get(kind.as_str()).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let Some(window) = value.as_object() else {
        // 非法窗口条目：拒收该条，不污染其他窗口。
        return Ok(None);
    };
    let prefix = kind.as_str();
    let Some(limit_usd) =
        optional_non_negative_f64(window.get("limit_usd"), &format!("{prefix}.limit_usd"))?
            .filter(|value| *value > 0.0)
    else {
        return Ok(None);
    };
    let Some(used_usd) =
        optional_non_negative_f64(window.get("used_usd"), &format!("{prefix}.used_usd"))
            .map(|value| value.unwrap_or(0.0))
            .ok()
    else {
        return Ok(None);
    };
    let Some(window_start_unix_secs) = optional_timestamp_unix_secs(
        window
            .get("window_start_unix_secs")
            .or_else(|| window.get("window_start")),
        &format!("{prefix}.window_start"),
    )?
    else {
        return Ok(None);
    };
    if window_start_unix_secs > now_unix_secs.saturating_add(MAX_FUTURE_WINDOW_START_SKEW_SECONDS) {
        return Ok(None);
    }
    let resets_at_unix_secs = optional_timestamp_unix_secs(
        window
            .get("resets_at_unix_secs")
            .or_else(|| window.get("resets_at")),
        &format!("{prefix}.resets_at"),
    )?
    .unwrap_or_else(|| {
        window_start_unix_secs.saturating_add(kind.interval_days().saturating_mul(DAY_SECONDS))
    });
    if resets_at_unix_secs <= window_start_unix_secs {
        return Ok(None);
    }

    Ok(Some(Sub2ApiRemoteQuotaWindow {
        kind,
        limit_usd,
        used_usd,
        window_start_unix_secs,
        resets_at_unix_secs,
    }))
}

fn successful_envelope_data<'a>(payload: &'a Value, label: &str) -> Result<&'a Value, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| format!("{label}响应必须是 JSON 对象"))?;
    if object.get("code").and_then(Value::as_i64) != Some(0) {
        // 上游 message 是不可信响应数据，可能回显凭据或请求头；这里只返回本地静态分类。
        return Err("业务状态码表示失败".to_string());
    }
    object
        .get("data")
        .ok_or_else(|| format!("{label}响应缺少 data"))
}

fn canonical_json_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
}

fn optional_non_negative_f64(value: Option<&Value>, field: &str) -> Result<Option<f64>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = value_as_f64(value).ok_or_else(|| format!("Sub2API {field} 必须是数字"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("Sub2API {field} 必须是有限的非负数"));
    }
    Ok(Some(value))
}

fn optional_timestamp_unix_secs(value: Option<&Value>, field: &str) -> Result<Option<u64>, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    if let Some(secs) = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
        })
    {
        return Ok(Some(secs));
    }
    let raw = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Sub2API {field} 必须是 Unix 秒或 RFC3339 时间"))?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(raw)
        .map_err(|_| format!("Sub2API {field} 必须是 Unix 秒或 RFC3339 时间"))?
        .timestamp();
    u64::try_from(timestamp)
        .map(Some)
        .map_err(|_| format!("Sub2API {field} 不能早于 Unix epoch"))
}

// ---------- 管理面只读投影（PR-B） ----------

const REMOTE_QUOTA_ADMIN_DISPLAY_STRING_MAX_CHARS: usize = 100;

/// 管理面只读投影：provider 的 remote_quota 配置与最近一次同步状态。
///
/// `key_status_snapshots` 为该 provider 各 key 的 `status_snapshot` 列值。
/// 写入侧已保证这些字段不含凭据；投影仍按白名单复制并对字符串做形态约束，
/// 时间字段统一转为 RFC3339（与 admin provider summary 既有风格一致）。
pub fn build_sub2api_remote_quota_admin_status(
    provider_config: Option<&Value>,
    key_status_snapshots: &[Option<&Value>],
) -> Value {
    let provider_ops_config = provider_config
        .and_then(Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(Value::as_object);
    let enabled = provider_ops_config
        .and_then(|ops| ops.get("remote_quota"))
        .and_then(Value::as_object)
        .and_then(|remote_quota| remote_quota.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return json!({ "enabled": false });
    }

    let (group_id, progress_endpoint, fetch_interval_seconds, config_error) =
        match provider_ops_config.map(parse_sub2api_remote_quota_config) {
            Some(Ok(Some(config))) => (
                Some(config.group_id),
                Some(config.progress_endpoint),
                Some(config.fetch_interval_seconds),
                Value::Null,
            ),
            Some(Err(message)) => (None, None, None, Value::String(message)),
            _ => (None, None, None, Value::Null),
        };
    let sync = key_status_snapshots
        .iter()
        .filter_map(|snapshot| snapshot.and_then(project_sub2api_remote_quota_sync_status))
        .max_by_key(|(sort_key, _)| *sort_key)
        .map(|(_, payload)| payload)
        .unwrap_or(Value::Null);

    json!({
        "enabled": true,
        "group_id": group_id,
        "progress_endpoint": progress_endpoint,
        "fetch_interval_seconds": fetch_interval_seconds,
        "config_error": config_error,
        "sync": sync,
    })
}

fn project_sub2api_remote_quota_sync_status(status_snapshot: &Value) -> Option<(u64, Value)> {
    let quota = status_snapshot.get("quota")?.as_object()?;
    let remote_quota = quota.get("remote_quota")?.as_object()?;
    let sync_status = safe_status_token(remote_quota.get("sync_status"))?;

    let synced_at = json_u64(remote_quota.get("synced_at_unix_secs"));
    let last_error_at = json_u64(remote_quota.get("last_error_at_unix_secs"));
    let blocked_until = json_u64(remote_quota.get("blocked_until_unix_secs"));
    let expires_at = json_u64(remote_quota.get("expires_at_unix_secs"));
    let windows = quota
        .get("windows")
        .and_then(Value::as_array)
        .map(|windows| {
            windows
                .iter()
                .filter_map(project_sub2api_remote_quota_window)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let payload = json!({
        "sync_status": sync_status,
        "synced_at": synced_at.and_then(unix_secs_to_rfc3339),
        "code": safe_status_token(quota.get("code")),
        "exhausted": quota.get("exhausted").and_then(Value::as_bool).unwrap_or(false),
        "usage_ratio": quota.get("usage_ratio").filter(|value| value.is_number()).cloned(),
        "reset_at": json_u64(quota.get("reset_at")).and_then(unix_secs_to_rfc3339),
        "updated_at": json_u64(quota.get("updated_at")).and_then(unix_secs_to_rfc3339),
        "blocked": remote_quota.get("blocked").and_then(Value::as_bool).unwrap_or(false),
        "block_reason": safe_status_token(remote_quota.get("block_reason")),
        "blocked_until": blocked_until.and_then(unix_secs_to_rfc3339),
        "conservative_cooldown": remote_quota
            .get("conservative_cooldown")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "subscription_id": safe_display_string(remote_quota.get("subscription_id")),
        "subscription_status": safe_status_token(remote_quota.get("subscription_status")),
        "subscription_active": remote_quota
            .get("subscription_active")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "group_id": safe_display_string(remote_quota.get("group_id")),
        "group_name": safe_display_string(remote_quota.get("group_name")),
        "expires_at": expires_at.and_then(unix_secs_to_rfc3339),
        "last_error": safe_display_string(remote_quota.get("last_error")),
        "last_error_at": last_error_at.and_then(unix_secs_to_rfc3339),
        "windows": windows,
    });
    let sort_key = synced_at.or(last_error_at).unwrap_or(0);
    Some((sort_key, payload))
}

fn project_sub2api_remote_quota_window(value: &Value) -> Option<Value> {
    let window = value.as_object()?;
    let code = safe_status_token(window.get("code"))?;
    Some(json!({
        "code": code,
        "label": safe_display_string(window.get("label")),
        "used_value": window.get("used_value").filter(|value| value.is_number()).cloned(),
        "limit_value": window.get("limit_value").filter(|value| value.is_number()).cloned(),
        "used_ratio": window.get("used_ratio").filter(|value| value.is_number()).cloned(),
        "remaining_value": window.get("remaining_value").filter(|value| value.is_number()).cloned(),
        "reset_at": json_u64(window.get("reset_at")).and_then(unix_secs_to_rfc3339),
        "reset_seconds": json_u64(window.get("reset_seconds")),
        "is_exhausted": window.get("is_exhausted").and_then(Value::as_bool).unwrap_or(false),
    }))
}

fn safe_status_token(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty()
        || raw.len() > 64
        || !raw
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return None;
    }
    Some(raw.to_string())
}

fn safe_display_string(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(
        raw.chars()
            .take(REMOTE_QUOTA_ADMIN_DISPLAY_STRING_MAX_CHARS)
            .collect(),
    )
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    value?
        .as_u64()
        .or_else(|| value?.as_i64().and_then(|value| u64::try_from(value).ok()))
}

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    let timestamp = i64::try_from(unix_secs).ok()?;
    Some(
        chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        build_sub2api_remote_quota_admin_status, parse_sub2api_remote_quota_at,
        parse_sub2api_remote_quota_config, validate_sub2api_same_origin_endpoint,
        Sub2ApiQuotaWindowKind, Sub2ApiRemoteQuotaState, Sub2ApiRemoteQuotaWindow,
        DEFAULT_REMOTE_QUOTA_FETCH_INTERVAL_SECS, MIN_REMOTE_QUOTA_FETCH_INTERVAL_SECS,
    };
    use serde_json::{json, Value};

    const TEST_NOW_UNIX_SECS: u64 = 1_896_004_800; // 2030-01-30T12:00:00Z

    fn parse_state(
        summary_json: &Value,
        progress_json: Option<&Value>,
        expected_group_id: &str,
    ) -> Result<Sub2ApiRemoteQuotaState, String> {
        parse_sub2api_remote_quota_at(
            summary_json,
            progress_json,
            expected_group_id,
            TEST_NOW_UNIX_SECS,
        )
    }

    fn summary() -> Value {
        json!({
            "code": 0,
            "data": {
                "subscriptions": [{
                    "id": 9,
                    "group_id": 42,
                    "group_name": "Pro",
                    "status": "active",
                    "monthly_used_usd": 12.5,
                    "monthly_limit_usd": 100,
                    "expires_at_unix_secs": 1_896_134_400
                }]
            }
        })
    }

    fn progress() -> Value {
        json!({
            "code": 0,
            "data": [{
                "subscription_id": 9,
                "group_id": 42,
                "expires_at_unix_secs": 1_896_134_400,
                "monthly": {
                    "limit_usd": 100,
                    "used_usd": 12.0,
                    "window_start_unix_secs": 1_893_456_000,
                    "resets_at_unix_secs": 1_896_048_000
                }
            }]
        })
    }

    fn active_windows(state: Sub2ApiRemoteQuotaState) -> Vec<Sub2ApiRemoteQuotaWindow> {
        match state {
            Sub2ApiRemoteQuotaState::Active(observation) => observation.windows,
            other => panic!("expected active observation, got {other:?}"),
        }
    }

    #[test]
    fn config_defaults_to_disabled() {
        let provider_ops = json!({}).as_object().cloned().expect("config");
        assert_eq!(
            parse_sub2api_remote_quota_config(&provider_ops).expect("empty config should parse"),
            None
        );
        let provider_ops = json!({"remote_quota": {"enabled": false, "group_id": "42"}})
            .as_object()
            .cloned()
            .expect("config");
        assert_eq!(
            parse_sub2api_remote_quota_config(&provider_ops).expect("disabled config should parse"),
            None
        );
    }

    #[test]
    fn config_parses_enabled_with_defaults() {
        let provider_ops = json!({"remote_quota": {"enabled": true, "group_id": 42}})
            .as_object()
            .cloned()
            .expect("config");
        let config = parse_sub2api_remote_quota_config(&provider_ops)
            .expect("config should parse")
            .expect("config should be enabled");
        assert_eq!(config.group_id, "42");
        assert_eq!(config.progress_endpoint, "/api/v1/subscriptions/progress");
        assert_eq!(
            config.fetch_interval_seconds,
            DEFAULT_REMOTE_QUOTA_FETCH_INTERVAL_SECS
        );
    }

    #[test]
    fn config_clamps_fetch_interval_to_floor() {
        let provider_ops = json!({
            "remote_quota": {"enabled": true, "group_id": "42", "fetch_interval_seconds": 5}
        })
        .as_object()
        .cloned()
        .expect("config");
        let config = parse_sub2api_remote_quota_config(&provider_ops)
            .expect("config should parse")
            .expect("config should be enabled");
        assert_eq!(
            config.fetch_interval_seconds,
            MIN_REMOTE_QUOTA_FETCH_INTERVAL_SECS
        );
    }

    #[test]
    fn config_rejects_invalid_values() {
        for remote_quota in [
            json!("enabled"),
            json!({"enabled": true}),
            json!({"enabled": true, "group_id": "  "}),
            json!({"enabled": true, "group_id": "42", "progress_endpoint": "https://evil.example/x"}),
            json!({"enabled": true, "group_id": "42", "progress_endpoint": "//evil.example/x"}),
            json!({"enabled": true, "group_id": "42", "progress_endpoint": "/api#frag"}),
            json!({"enabled": true, "group_id": "42", "fetch_interval_seconds": 0}),
            json!({"enabled": true, "group_id": "42", "fetch_interval_seconds": -5}),
            json!({"enabled": true, "group_id": "42", "fetch_interval_seconds": "300"}),
        ] {
            let provider_ops = json!({"remote_quota": remote_quota})
                .as_object()
                .cloned()
                .expect("config");
            assert!(
                parse_sub2api_remote_quota_config(&provider_ops).is_err(),
                "config should be rejected: {provider_ops:?}"
            );
        }
    }

    #[test]
    fn config_tolerates_unknown_fields() {
        let provider_ops = json!({
            "remote_quota": {
                "enabled": true,
                "group_id": "42",
                "sync_mode": "overwrite_local_quota",
                "future_field": {"nested": true}
            }
        })
        .as_object()
        .cloned()
        .expect("config");
        assert!(parse_sub2api_remote_quota_config(&provider_ops)
            .expect("unknown fields should be tolerated")
            .is_some());
    }

    #[test]
    fn same_origin_endpoint_validation() {
        assert!(validate_sub2api_same_origin_endpoint("/api/v1/subscriptions/progress").is_ok());
        for endpoint in [
            "",
            "api/v1/progress",
            "//evil.example/x",
            "https://evil.example/x",
            "/api#frag",
            "/api\\..\\admin",
        ] {
            assert!(
                validate_sub2api_same_origin_endpoint(endpoint).is_err(),
                "endpoint should be rejected: {endpoint}"
            );
        }
    }

    #[test]
    fn parses_flat_contract_monthly_window() {
        let state =
            parse_state(&summary(), Some(&progress()), "42").expect("remote quota should parse");
        let Sub2ApiRemoteQuotaState::Active(observation) = state else {
            panic!("expected active observation");
        };
        assert_eq!(observation.subscription_id, "9");
        assert_eq!(observation.group_id, "42");
        assert_eq!(observation.group_name, "Pro");
        assert_eq!(observation.expires_at_unix_secs, Some(1_896_134_400));
        assert_eq!(observation.windows.len(), 1);
        let window = &observation.windows[0];
        assert_eq!(window.kind, Sub2ApiQuotaWindowKind::Monthly);
        assert_eq!(window.limit_usd, 100.0);
        assert_eq!(window.used_usd, 12.0);
        assert_eq!(window.window_start_unix_secs, 1_893_456_000);
        assert_eq!(window.resets_at_unix_secs, 1_896_048_000);
        assert!(!window.is_exhausted_at(TEST_NOW_UNIX_SECS));
    }

    #[test]
    fn parses_nested_upstream_contract_and_rfc3339_timestamps() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9,
                "group": {"id": 42, "name": "Pro", "monthly_limit_usd": 100},
                "status": "active",
                "monthly_used_usd": 12,
                "expires_at": "2030-02-01T00:00:00Z"
            }]}
        });
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription": {"id": 9, "group_id": 42},
                "progress": {
                    "expires_at": "2030-02-01T00:00:00Z",
                    "monthly": {
                        "limit_usd": 100,
                        "used_usd": 12,
                        "window_start": "2030-01-01T00:00:00Z",
                        "resets_at": "2030-01-31T00:00:00Z"
                    }
                }
            }]
        });

        let state =
            parse_state(&summary, Some(&progress), "42").expect("nested contract should parse");
        let Sub2ApiRemoteQuotaState::Active(observation) = state else {
            panic!("expected active observation");
        };
        assert_eq!(observation.group_name, "Pro");
        assert_eq!(observation.windows.len(), 1);
        assert_eq!(observation.windows[0].resets_at_unix_secs, 1_896_048_000);
        assert_eq!(observation.expires_at_unix_secs, Some(1_896_134_400));
    }

    #[test]
    fn unlimited_subscription_needs_no_progress() {
        let state = parse_state(
            &json!({
                "code": 0,
                "data": {"subscriptions": [{
                    "id": "sub-1", "group_id": "group-1", "status": "active"
                }]}
            }),
            None,
            "group-1",
        )
        .expect("unlimited quota should parse");
        assert!(matches!(
            state,
            Sub2ApiRemoteQuotaState::Active(ref observation) if observation.windows.is_empty()
        ));
    }

    #[test]
    fn missing_group_is_subscription_invalid() {
        let state = parse_state(&summary(), Some(&progress()), "99")
            .expect("missing group should classify");
        assert_eq!(
            state,
            Sub2ApiRemoteQuotaState::SubscriptionInvalid {
                group_id: "99".to_string(),
                subscription_status: None,
                expires_at_unix_secs: None,
            }
        );
    }

    #[test]
    fn non_active_or_expired_subscription_is_invalid() {
        let expired = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9, "group_id": 42, "status": "active",
                "expires_at_unix_secs": 1_600_000_000
            }]}
        });
        assert!(matches!(
            parse_state(&expired, None, "42").expect("expired should classify"),
            Sub2ApiRemoteQuotaState::SubscriptionInvalid {
                subscription_status: Some(ref status),
                ..
            } if status == "active"
        ));

        let disabled = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9, "group_id": 42, "status": "disabled",
                "expires_at_unix_secs": 1_996_134_400
            }]}
        });
        assert_eq!(
            parse_state(&disabled, None, "42").expect("disabled should classify"),
            Sub2ApiRemoteQuotaState::SubscriptionInvalid {
                group_id: "42".to_string(),
                subscription_status: Some("disabled".to_string()),
                expires_at_unix_secs: Some(1_996_134_400),
            }
        );
    }

    #[test]
    fn exhausted_window_classification() {
        let mut progress = progress();
        progress["data"][0]["monthly"]["used_usd"] = json!(100.0);
        let state = parse_state(&summary(), Some(&progress), "42").expect("should parse");
        let windows = active_windows(state);
        assert!(windows[0].is_exhausted_at(TEST_NOW_UNIX_SECS));
        assert!(!windows[0].is_exhausted_at(1_896_048_000));
        assert!(!windows[0].is_exhausted_at(1_896_048_001));
    }

    #[test]
    fn selects_multiple_limited_windows() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9, "group_id": 42, "status": "active",
                "daily_limit_usd": 10, "weekly_limit_usd": 50, "monthly_limit_usd": 0
            }]}
        });
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription_id": 9, "group_id": 42,
                "daily": {
                    "limit_usd": 10, "used_usd": 10,
                    "window_start_unix_secs": 1_895_961_600, "resets_at_unix_secs": 1_896_048_000
                },
                "weekly": {
                    "limit_usd": 50, "used_usd": 7,
                    "window_start_unix_secs": 1_895_443_200, "resets_at_unix_secs": 1_896_048_000
                }
            }]
        });
        let state = parse_state(&summary, Some(&progress), "42").expect("should parse");
        let windows = active_windows(state);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, Sub2ApiQuotaWindowKind::Daily);
        assert!(windows[0].is_exhausted_at(TEST_NOW_UNIX_SECS));
        assert!(!windows[1].is_exhausted_at(TEST_NOW_UNIX_SECS));
    }

    #[test]
    fn rejects_duplicate_active_subscriptions_for_one_group() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [
                {"id": 9, "group_id": 42, "status": "active", "monthly_limit_usd": 100},
                {"id": 10, "group_id": "42", "status": "active", "monthly_limit_usd": 100}
            ]}
        });
        let error = parse_state(&summary, None, "42").expect_err("duplicates must fail");
        assert!(error.contains("多个活跃套餐"));
    }

    #[test]
    fn limited_quota_requires_progress_response() {
        let error = parse_state(&summary(), None, "42").expect_err("limited quota needs progress");
        assert!(error.contains("subscriptions/progress"));
    }

    #[test]
    fn limited_quota_requires_matching_progress_record() {
        let progress = json!({"code": 0, "data": []});
        let error = parse_state(&summary(), Some(&progress), "42")
            .expect_err("missing progress record must fail");
        assert!(error.contains("progress 数据缺失"));

        let progress = json!({
            "code": 0,
            "data": [{"subscription_id": 9, "group_id": 42}]
        });
        let error = parse_state(&summary(), Some(&progress), "42")
            .expect_err("missing monthly window must fail");
        assert!(error.contains("没有可用的当前额度窗口"));
    }

    #[test]
    fn rejects_duplicate_matching_progress_records() {
        let mut duplicate = progress();
        let item = duplicate["data"][0].clone();
        duplicate["data"]
            .as_array_mut()
            .expect("progress data should be an array")
            .push(item);
        let error = parse_state(&summary(), Some(&duplicate), "42")
            .expect_err("duplicate progress must fail");
        assert!(error.contains("多个匹配的 progress"));
    }

    #[test]
    fn rejects_summary_progress_limit_mismatch() {
        let mut progress = progress();
        progress["data"][0]["monthly"]["limit_usd"] = json!(120.0);
        let error =
            parse_state(&summary(), Some(&progress), "42").expect_err("limit mismatch must fail");
        assert!(error.contains("不一致"));
    }

    #[test]
    fn drops_invalid_summary_entries_and_errors_when_none_usable() {
        // 单条坏数据：负值、NaN、缺 id、缺 status → 拒收该条；整体无可用条目 → 错误。
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [
                {"id": 1, "group_id": 42, "status": "active", "monthly_limit_usd": -5},
                {"id": 2, "group_id": 42, "status": "active", "monthly_used_usd": "NaN"},
                {"group_id": 42, "status": "active"},
                {"id": 4, "group_id": 42},
                "not-an-object"
            ]}
        });
        let error = parse_state(&summary, None, "42").expect_err("no usable entries must fail");
        assert!(error.contains("没有可用条目"));

        // 坏条目与好条目混合：坏条目被拒收，好条目照常生效。
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [
                {"id": 1, "group_id": 42, "status": "active", "monthly_limit_usd": -5},
                {"id": 9, "group_id": 42, "status": "active", "unknown_field": 1}
            ]}
        });
        let state = parse_state(&summary, None, "42").expect("usable entry should survive");
        assert!(matches!(state, Sub2ApiRemoteQuotaState::Active(_)));
    }

    #[test]
    fn drops_invalid_progress_windows_and_defaults_resets_at() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9, "group_id": 42, "status": "active",
                "daily_limit_usd": 10, "monthly_limit_usd": 100
            }]}
        });
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription_id": 9, "group_id": 42,
                "daily": {
                    "limit_usd": 10, "used_usd": 1,
                    "window_start_unix_secs": 1_895_961_600,
                    "resets_at_unix_secs": 1_895_961_600
                },
                "monthly": {
                    "limit_usd": 100, "used_usd": 12,
                    "window_start_unix_secs": 1_893_456_000
                }
            }]
        });
        // daily.resets_at <= window_start → 拒收该窗口 → summary 有限窗口缺 progress → 整体失败。
        let error = parse_state(&summary, Some(&progress), "42")
            .expect_err("dropped daily window must fail closed");
        assert!(error.contains("没有可用的当前额度窗口"));

        // 去掉 daily 限额后 monthly 生效；resets_at 缺失时按窗口长度推导。
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9, "group_id": 42, "status": "active", "monthly_limit_usd": 100
            }]}
        });
        let state = parse_state(&summary, Some(&progress), "42").expect("monthly should parse");
        let windows = active_windows(state);
        assert_eq!(
            windows[0].resets_at_unix_secs,
            1_893_456_000 + 30 * 24 * 60 * 60
        );
    }

    #[test]
    fn drops_progress_windows_with_bad_values_or_future_start() {
        let summary = json!({
            "code": 0,
            "data": {"subscriptions": [{
                "id": 9, "group_id": 42, "status": "active", "monthly_limit_usd": 100
            }]}
        });
        for window in [
            json!({"used_usd": 1, "window_start_unix_secs": 1_893_456_000}), // 缺 limit
            json!({"limit_usd": 0, "window_start_unix_secs": 1_893_456_000}), // limit 非正
            json!({"limit_usd": 100, "used_usd": -1, "window_start_unix_secs": 1_893_456_000}),
            json!({"limit_usd": 100, "used_usd": "NaN", "window_start_unix_secs": 1_893_456_000}),
            json!({"limit_usd": 100}), // 缺 window_start
            json!({"limit_usd": 100, "window_start_unix_secs": 1_896_004_800 + 600}), // 未来窗口起点
        ] {
            let progress = json!({
                "code": 0,
                "data": [{"subscription_id": 9, "group_id": 42, "monthly": window}]
            });
            let error = parse_state(&summary, Some(&progress), "42")
                .expect_err("invalid window must be dropped and fail closed");
            assert!(
                error.contains("没有可用的当前额度窗口"),
                "unexpected error: {error}"
            );
        }
        // 5 分钟内的时钟偏差容忍。
        let progress = json!({
            "code": 0,
            "data": [{
                "subscription_id": 9, "group_id": 42,
                "monthly": {
                    "limit_usd": 100, "used_usd": 0,
                    "window_start_unix_secs": 1_896_004_800 + 120,
                    "resets_at_unix_secs": 1_896_048_000
                }
            }]
        });
        assert!(parse_state(&summary, Some(&progress), "42").is_ok());
    }

    #[test]
    fn envelope_failures_do_not_echo_upstream_messages() {
        let secret = "authorization=Bearer upstream-secret";
        let error = parse_state(&json!({"code": 401, "message": secret}), None, "42")
            .expect_err("non-zero code must fail");
        assert_eq!(error, "业务状态码表示失败");
        assert!(!error.contains("upstream-secret"));

        let error =
            parse_state(&json!({"code": 0}), None, "42").expect_err("missing data must fail");
        assert!(error.contains("缺少 data"));

        let error =
            parse_state(&json!([1, 2, 3]), None, "42").expect_err("non-object payload must fail");
        assert!(error.contains("JSON 对象"));
    }

    #[test]
    fn progress_data_accepts_single_object_shape() {
        let progress = json!({
            "code": 0,
            "data": {
                "subscription_id": 9,
                "group_id": 42,
                "expires_at_unix_secs": 1_896_134_400,
                "monthly": {
                    "limit_usd": 100,
                    "used_usd": 12.0,
                    "window_start_unix_secs": 1_893_456_000,
                    "resets_at_unix_secs": 1_896_048_000
                }
            }
        });
        let state = parse_state(&summary(), Some(&progress), "42")
            .expect("single object progress should parse");
        assert_eq!(active_windows(state).len(), 1);
    }

    #[test]
    fn usage_ratio_helpers() {
        let window = Sub2ApiRemoteQuotaWindow {
            kind: Sub2ApiQuotaWindowKind::Weekly,
            limit_usd: 50.0,
            used_usd: 25.0,
            window_start_unix_secs: 1,
            resets_at_unix_secs: 2,
        };
        assert_eq!(window.used_ratio(), 0.5);
        let window = Sub2ApiRemoteQuotaWindow {
            used_usd: 75.0,
            ..window
        };
        assert_eq!(window.used_ratio(), 1.0);
    }

    fn enabled_provider_config() -> Value {
        json!({
            "provider_ops": {
                "architecture_id": "sub2api",
                "remote_quota": {
                    "enabled": true,
                    "group_id": "42",
                    "fetch_interval_seconds": 120,
                    "connector": {"credentials": {"refresh_token": "secret-token"}}
                }
            }
        })
    }

    fn synced_status_snapshot(synced_at: u64) -> Value {
        json!({
            "quota": {
                "version": 2,
                "provider_type": "sub2api",
                "code": "exhausted",
                "exhausted": true,
                "usage_ratio": 1.0,
                "updated_at": synced_at,
                "reset_at": 1_896_048_000_u64,
                "windows": [{
                    "code": "monthly",
                    "label": "月",
                    "used_value": 100.0,
                    "limit_value": 100.0,
                    "used_ratio": 1.0,
                    "remaining_value": 0.0,
                    "reset_at": 1_896_048_000_u64,
                    "reset_seconds": 43_200,
                    "is_exhausted": true,
                    "raw_internal": "drop-me"
                }],
                "remote_quota": {
                    "sync_status": "ok",
                    "synced_at_unix_secs": synced_at,
                    "group_id": "42",
                    "group_name": "Pro",
                    "subscription_id": "9",
                    "subscription_status": "active",
                    "subscription_active": true,
                    "expires_at_unix_secs": 1_896_134_400_u64,
                    "blocked": true,
                    "block_reason": "window_exhausted",
                    "blocked_until_unix_secs": 1_896_048_000_u64,
                    "conservative_cooldown": false,
                    "last_error": Value::Null
                }
            },
            "oauth": {"code": "none"}
        })
    }

    #[test]
    fn admin_status_disabled_by_default() {
        assert_eq!(
            build_sub2api_remote_quota_admin_status(None, &[]),
            json!({"enabled": false})
        );
        assert_eq!(
            build_sub2api_remote_quota_admin_status(Some(&json!({"provider_ops": {}})), &[]),
            json!({"enabled": false})
        );
        assert_eq!(
            build_sub2api_remote_quota_admin_status(
                Some(
                    &json!({"provider_ops": {"remote_quota": {"enabled": false, "group_id": "42"}}})
                ),
                &[]
            ),
            json!({"enabled": false})
        );
    }

    #[test]
    fn admin_status_projects_config_and_latest_sync() {
        let older = synced_status_snapshot(1_896_000_000);
        let newer = synced_status_snapshot(1_896_004_000);
        let status = build_sub2api_remote_quota_admin_status(
            Some(&enabled_provider_config()),
            &[Some(&older), Some(&newer), None],
        );
        assert_eq!(status["enabled"], json!(true));
        assert_eq!(status["group_id"], json!("42"));
        assert_eq!(
            status["progress_endpoint"],
            json!("/api/v1/subscriptions/progress")
        );
        assert_eq!(status["fetch_interval_seconds"], json!(120));
        assert_eq!(status["config_error"], json!(Value::Null));
        let sync = &status["sync"];
        assert_eq!(sync["sync_status"], json!("ok"));
        assert_eq!(sync["synced_at"], json!("2030-01-30T11:46:40Z"));
        assert_eq!(sync["code"], json!("exhausted"));
        assert_eq!(sync["exhausted"], json!(true));
        assert_eq!(sync["blocked"], json!(true));
        assert_eq!(sync["block_reason"], json!("window_exhausted"));
        assert_eq!(sync["blocked_until"], json!("2030-01-31T00:00:00Z"));
        assert_eq!(sync["subscription_id"], json!("9"));
        assert_eq!(sync["subscription_active"], json!(true));
        assert_eq!(sync["group_name"], json!("Pro"));
        assert_eq!(sync["windows"].as_array().expect("windows").len(), 1);
        assert_eq!(sync["windows"][0]["used_value"], json!(100.0));
        assert_eq!(
            sync["windows"][0]["reset_at"],
            json!("2030-01-31T00:00:00Z")
        );
        // 白名单之外的字段不得透出
        assert!(sync["windows"][0].get("raw_internal").is_none());
        let serialized = status.to_string();
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("refresh_token"));
        assert!(!serialized.contains("drop-me"));
    }

    #[test]
    fn admin_status_without_sync_history_exposes_null_sync() {
        let status = build_sub2api_remote_quota_admin_status(
            Some(&enabled_provider_config()),
            &[None, Some(&json!({"oauth": {"code": "none"}}))],
        );
        assert_eq!(status["enabled"], json!(true));
        assert_eq!(status["sync"], json!(Value::Null));
    }

    #[test]
    fn admin_status_surfaces_invalid_config_error() {
        let status = build_sub2api_remote_quota_admin_status(
            Some(&json!({
                "provider_ops": {"remote_quota": {"enabled": true, "group_id": "42", "fetch_interval_seconds": 0}}
            })),
            &[],
        );
        assert_eq!(status["enabled"], json!(true));
        assert!(status["config_error"]
            .as_str()
            .is_some_and(|message| message.contains("fetch_interval_seconds")));
        assert_eq!(status["sync"], json!(Value::Null));
    }

    #[test]
    fn admin_status_prefers_error_snapshot_when_it_is_the_latest() {
        let ok_snapshot = synced_status_snapshot(1_896_000_000);
        let error_snapshot = json!({
            "quota": {
                "code": "exhausted",
                "exhausted": true,
                "windows": [],
                "remote_quota": {
                    "sync_status": "error",
                    "synced_at_unix_secs": 1_896_000_000_u64,
                    "group_id": "42",
                    "last_error": "网络错误",
                    "last_error_at_unix_secs": 1_896_004_500_u64
                }
            }
        });
        let status = build_sub2api_remote_quota_admin_status(
            Some(&enabled_provider_config()),
            &[Some(&ok_snapshot), Some(&error_snapshot)],
        );
        assert_eq!(status["sync"]["sync_status"], json!("error"));
        assert_eq!(status["sync"]["last_error"], json!("网络错误"));
        assert_eq!(
            status["sync"]["last_error_at"],
            json!("2030-01-30T11:55:00Z")
        );
    }

    #[test]
    fn admin_status_drops_unsafe_tokens_and_oversized_strings() {
        let mut snapshot = synced_status_snapshot(1_896_000_000);
        snapshot["quota"]["remote_quota"]["sync_status"] = json!("ok; DROP TABLE");
        snapshot["quota"]["remote_quota"]["group_name"] = json!("x".repeat(500));
        snapshot["quota"]["code"] = json!("exhausted\r\ninjected");
        let status = build_sub2api_remote_quota_admin_status(
            Some(&enabled_provider_config()),
            &[Some(&snapshot)],
        );
        // sync_status 非法 → 整条快照不可投影，sync 为 null
        assert_eq!(status["sync"], json!(Value::Null));

        snapshot["quota"]["remote_quota"]["sync_status"] = json!("ok");
        let status = build_sub2api_remote_quota_admin_status(
            Some(&enabled_provider_config()),
            &[Some(&snapshot)],
        );
        assert_eq!(status["sync"]["code"], json!(Value::Null));
        assert_eq!(
            status["sync"]["group_name"]
                .as_str()
                .expect("group_name")
                .chars()
                .count(),
            100
        );
    }
}
