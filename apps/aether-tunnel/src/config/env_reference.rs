//! Generate and check the operator reference from the actual clap command.
//! This module is compiled only for tests; it never parses or displays env values.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::PathBuf;

use clap::{Arg, CommandFactory};

use super::Config;

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ENVIRONMENT.md")
}

fn cell(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}

fn unit(arg: &Arg) -> &'static str {
    let id = arg.get_id().as_str();
    if id.ends_with("_ms") {
        return "毫秒";
    }
    if id.ends_with("_secs") || id == "heartbeat_interval" {
        return "秒";
    }
    if id.ends_with("_bytes") || id == "legacy_redirect_replay_budget_bytes_ignored" {
        return "字节";
    }
    if id.ends_with("_percent") {
        return "%";
    }
    match id {
        "log_retention_days" => "天",
        "aether_pool_max_idle_per_host"
        | "upstream_pool_max_idle_per_host"
        | "max_concurrent_connections"
        | "tunnel_connections"
        | "tunnel_connections_max" => "连接数",
        "max_in_flight_streams" | "distributed_stream_limit" | "tunnel_max_streams" => "stream 数",
        "dns_cache_capacity" => "条目数",
        "upstream_client_pool_capacity" => "client 数",
        "aether_retry_max_attempts" => "尝试次数（含首次）",
        "log_max_files" => "文件数",
        "allowed_ports" => "端口号（逗号分隔）",
        "aether_url"
        | "aether_outbound_proxy_url"
        | "upstream_proxy_url"
        | "distributed_stream_redis_url" => "URL",
        "diagnostics_bind" => "IP:端口",
        "public_ip" => "IP 地址",
        "log_dir" => "路径",
        "tunnel_encryption_key" => "base64（32 字节 PSK）",
        "management_token"
        | "node_name"
        | "node_region"
        | "distributed_stream_redis_key_prefix"
        | "log_level"
        | "log_destination"
        | "log_rotation"
        | "tunnel_profile"
        | "tunnel_security" => "字符串",
        "allow_private_targets"
        | "remote_upgrade_enabled"
        | "aether_tcp_nodelay"
        | "aether_http2"
        | "upstream_tcp_nodelay"
        | "upstream_proxy_remote_dns"
        | "emit_proxy_timing_header"
        | "tunnel_ipv4_only"
        | "tunnel_ipv6_only"
        | "tunnel_tcp_nodelay" => "布尔值",
        _ => panic!("document the unit for new clap argument {id}"),
    }
}

fn render_reference() -> String {
    let mut output = String::from(
        "# Tunnel 环境变量参考\n\n\
         本文件由实际 `Config::command()` 的 clap 元数据生成。不要手工编辑；更新命令：\n\n\
         ```bash\n\
         cargo test -p aether-tunnel --bin aether-tunnel config::env_reference::regenerate_tunnel_env_reference -- --ignored --exact\n\
         ```\n\n\
         校验命令：`cargo test -p aether-tunnel --bin aether-tunnel config::env_reference`。\n\
         Rust CI 的 `Test (Workspace Rest)` 会运行同一校验；环境变量名、CLI 名、默认值、说明或枚举值变化都会要求重新生成。\n\n\
         ## 读取规则\n\n\
         - 默认值列只导出 clap 声明的默认值，不导出当前环境变量值或实际凭据。CLI 覆盖环境变量，环境变量覆盖 TOML。\n\
         - `必填` 表示 clap 无默认值；直接运行需提供值，使用 TOML 时由配置加载器提供。多服务器的 URL、Token 和可选节点覆盖放在 `[[servers]]` 中。\n\
         - `未设置` 表示 clap 的 `Option` 为 `None`，不等于零或一律禁用。公网 IP、地区、并发及连接池有启动时探测或推导；详见 [README](README.md#配置) 和 `src/app.rs` / `Config::resolve_tunnel_pool_sizing`。\n\
         - 省略 `diagnostics_bind` 不启动诊断监听；省略 `distributed_stream_limit` 不启用跨实例 admission，启用时必须同时配置 Redis URL。省略两个出口 proxy URL 时对应流量直连。\n\
         - 此表覆盖全部 clap 环境变量，包括标记为隐藏的兼容参数。兼容参数只接受输入，不生效；不能用于限制资源。\n\
         - 秒、毫秒和字节以单位列为准，不要根据环境变量是否带 `_SECS` 猜测。clap 不会为拼错的名称自动创建别名，未知环境变量可能被忽略。\n\n\
         ## 启动和安装脚本变量\n\n\
         以下变量不属于 clap 参数，分别由启动入口和安装脚本读取，因此不在生成表内：\n\n\
         | 环境变量 | 默认或省略行为 | 读取方 |\n\
         | --- | --- | --- |\n\
         | `AETHER_TUNNEL_CONFIG` | `aether-tunnel.toml`（路径） | `src/main.rs`；安装脚本也接受配置路径 |\n\
         | `AETHER_TUNNEL_RELEASE_TAG` | 自动选择最新 tunnel tag | `install.sh` / `install.ps1` |\n\
         | `AETHER_TUNNEL_INSTALL_DIR` | 按系统选择安装目录 | `install.sh` / `install.ps1` |\n\n\
         ## clap 参数\n\n\
         | 环境变量 | CLI 参数 | clap 默认值 | 单位 / 类型 | 说明 |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    let command = Config::command();
    let mut args: Vec<_> = command
        .get_arguments()
        .filter(|arg| arg.get_env().is_some())
        .collect();
    args.sort_by_key(|arg| arg.get_env().unwrap());
    for arg in args {
        let defaults = arg
            .get_default_values()
            .iter()
            .map(|value| value.to_str().expect("UTF-8 clap default"))
            .collect::<Vec<_>>()
            .join(",");
        let default = if arg.get_default_values().is_empty() {
            if arg.is_required_set() {
                "必填".to_owned()
            } else {
                "未设置".to_owned()
            }
        } else if defaults.is_empty() {
            "空字符串".to_owned()
        } else {
            format!("`{}`", cell(&defaults))
        };
        let mut help = cell(
            &arg.get_long_help()
                .or_else(|| arg.get_help())
                .expect("clap env argument must have documentation")
                .to_string(),
        );
        if arg.get_id() == "legacy_redirect_replay_budget_bytes_ignored" {
            help.insert_str(0, "**隐藏兼容参数；输入被忽略。** ");
        } else if arg.is_hide_set() {
            help.insert_str(0, "**隐藏参数。** ");
        }
        if let Some(values) = arg.get_value_parser().possible_values() {
            let values = values
                .filter(|value| !value.is_hide_set())
                .map(|value| format!("`{}`", cell(value.get_name())))
                .collect::<Vec<_>>();
            if !values.is_empty() {
                write!(help, " 可选值：{}。", values.join(", ")).unwrap();
            }
        }
        writeln!(
            output,
            "| `{}` | `--{}` | {} | {} | {} |",
            arg.get_env().unwrap().to_str().expect("UTF-8 env name"),
            arg.get_long()
                .expect("clap env argument must have a long flag"),
            default,
            unit(arg),
            help,
        )
        .unwrap();
    }
    output
}

#[test]
fn tunnel_env_reference_matches_clap() {
    let expected = render_reference();
    let actual = std::fs::read_to_string(reference_path())
        .expect("ENVIRONMENT.md must exist; run the documented regeneration command");
    assert_eq!(
        actual, expected,
        "ENVIRONMENT.md has drifted; run the regeneration command at its top and review the diff"
    );
}

#[test]
fn readme_uses_known_tunnel_env_names() {
    let command = Config::command();
    let mut known: BTreeSet<_> = command
        .get_arguments()
        .filter_map(|arg| arg.get_env().map(|env| env.to_str().unwrap()))
        .collect();
    // These are intentionally outside clap; keep ownership explicit.
    known.extend([
        "AETHER_TUNNEL_CONFIG",
        "AETHER_TUNNEL_RELEASE_TAG",
        "AETHER_TUNNEL_INSTALL_DIR",
    ]);
    let readme = include_str!("../../README.md");
    for word in
        readme.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
    {
        if word.starts_with("AETHER_TUNNEL_") && word != "AETHER_TUNNEL_" {
            assert!(
                known.contains(word),
                "README uses unknown environment variable {word}"
            );
        }
    }
    assert!(readme.contains("[完整环境变量参考](ENVIRONMENT.md)"));
}

#[test]
#[ignore = "explicit documentation regeneration; the normal test checks for drift"]
fn regenerate_tunnel_env_reference() {
    std::fs::write(reference_path(), render_reference()).expect("write ENVIRONMENT.md");
}
