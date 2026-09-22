use std::time::Duration;

use aether_loadtools::{
    run_http_load_probe_with_options, HttpLoadProbeConfig, HttpLoadProbeOptions,
};
use clap::Parser;
use reqwest::Method;

#[derive(Parser)]
#[command(
    name = "http_load_probe",
    about = "Standalone HTTP load probe",
    long_about = "usage: cargo run -p aether-loadtools --bin http_load_probe -- --url <URL> --requests <N> --concurrency <N> [options]"
)]
struct Cli {
    /// Target URL.
    #[arg(long)]
    url: String,
    /// Optional warmup URL probed before the measured run.
    #[arg(long)]
    warmup_url: Option<String>,
    /// Total number of requests.
    #[arg(long)]
    requests: usize,
    /// Concurrent in-flight requests.
    #[arg(long)]
    concurrency: usize,
    /// Connections opened during warmup.
    #[arg(long, default_value_t = 0)]
    warmup_connections: usize,
    /// Per-request timeout in milliseconds.
    #[arg(long)]
    timeout_ms: Option<u64>,
    /// TCP connect timeout in milliseconds.
    #[arg(long)]
    connect_timeout_ms: Option<u64>,
    /// Client shards partitioning the connection pool.
    #[arg(long)]
    client_shards: Option<usize>,
    /// Max idle connections per host in the pool.
    #[arg(long)]
    pool_max_idle_per_host: Option<usize>,
    /// Ramp-up duration before full concurrency in milliseconds.
    #[arg(long, default_value_t = 0)]
    start_ramp_ms: u64,
    /// Hold applied to the first response body chunk in milliseconds.
    #[arg(long, default_value_t = 0)]
    first_body_hold_ms: u64,
    /// Force HTTP/1.1 only.
    #[arg(long)]
    http1_only: bool,
    /// Use HTTP/2 prior knowledge.
    #[arg(long)]
    http2_prior_knowledge: bool,
    /// HTTP method.
    #[arg(long, default_value = "GET")]
    method: Method,
    /// Header in `Name: value` or `Name=value` form; repeatable.
    #[arg(long = "header", short = 'H', value_parser = parse_header_pair)]
    headers: Vec<(String, String)>,
    /// Request body.
    #[arg(long, conflicts_with = "body_file")]
    body: Option<String>,
    /// Read the request body from a file.
    #[arg(long)]
    body_file: Option<std::path::PathBuf>,
    /// Response consumption mode.
    #[arg(long, default_value = "headers", value_parser = parse_response_mode)]
    response_mode: aether_loadtools::HttpLoadProbeResponseMode,
    /// Fail the probe unless SSE streams end with [DONE].
    #[arg(long)]
    require_sse_done: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (config, options) = parse_args(Cli::parse())?;
    let result = run_http_load_probe_with_options(&config, options)
        .await
        .map_err(std::io::Error::other)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn parse_args(
    cli: Cli,
) -> Result<(HttpLoadProbeConfig, HttpLoadProbeOptions), Box<dyn std::error::Error>> {
    let body = match (cli.body, cli.body_file) {
        (Some(body), None) => Some(body.into_bytes()),
        (None, Some(path)) => Some(std::fs::read(path)?),
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!("clap conflict enforcement"),
    };
    let mut config = HttpLoadProbeConfig {
        url: cli.url,
        total_requests: cli.requests,
        concurrency: cli.concurrency,
        method: cli.method,
        headers: cli.headers.into_iter().collect(),
        body,
        response_mode: cli.response_mode,
        ..HttpLoadProbeConfig::default()
    };
    config.warmup_url = cli.warmup_url;
    config.warmup_connections = cli.warmup_connections;
    if let Some(timeout_ms) = cli.timeout_ms {
        config.timeout = Duration::from_millis(timeout_ms);
    }
    config.connect_timeout = cli.connect_timeout_ms.map(Duration::from_millis);
    if let Some(client_shards) = cli.client_shards {
        config.client_shards = client_shards;
    }
    config.pool_max_idle_per_host = cli.pool_max_idle_per_host;
    config.start_ramp = Duration::from_millis(cli.start_ramp_ms);
    config.first_body_hold = Duration::from_millis(cli.first_body_hold_ms);
    config.http1_only = cli.http1_only;
    config.http2_prior_knowledge = cli.http2_prior_knowledge;
    config
        .validate()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    Ok((
        config,
        HttpLoadProbeOptions {
            require_sse_done: cli.require_sse_done,
        },
    ))
}

fn parse_header_pair(value: &str) -> Result<(String, String), std::io::Error> {
    let (name, value) = value
        .split_once(':')
        .or_else(|| value.split_once('='))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--header expects `Name: value` or `Name=value`",
            )
        })?;
    let name = name.trim();
    if name.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--header name cannot be empty",
        ));
    }
    Ok((name.to_string(), value.trim().to_string()))
}

fn parse_response_mode(
    value: &str,
) -> Result<aether_loadtools::HttpLoadProbeResponseMode, std::io::Error> {
    match value.trim().to_ascii_lowercase().as_str() {
        "headers" | "headers-only" | "header" => {
            Ok(aether_loadtools::HttpLoadProbeResponseMode::HeadersOnly)
        }
        "first-body-byte" | "first-body" | "first-byte" | "first-chunk" => {
            Ok(aether_loadtools::HttpLoadProbeResponseMode::FirstBodyByte)
        }
        "full" | "full-body" | "body" => Ok(aether_loadtools::HttpLoadProbeResponseMode::FullBody),
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "unsupported --response-mode {other}; expected headers, first-body-byte, or full"
            ),
        )),
    }
}
