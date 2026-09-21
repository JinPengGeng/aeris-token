//! Upstream WebSocket handshake and frame conversion utilities.
//!
//! These helpers intentionally do not parse messages.  A protocol adapter is
//! responsible for deciding when and what to send, while this module owns the
//! HTTP-to-WebSocket transport conversion and provider transport profile.

use std::collections::BTreeMap;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use aether_contracts::ProxySnapshot;
use axum::extract::ws::{CloseFrame as AxumCloseFrame, Message as AxumWsMessage, WebSocket};
use axum::http::header::{
    ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HOST,
    PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use axum::http::{HeaderMap, HeaderName};
use base64::Engine as _;
use bytes::Bytes;
use futures_util::stream::{Stream, StreamExt};
use futures_util::{Sink, SinkExt, TryFutureExt};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use wreq::ws::message::{CloseFrame as WreqCloseFrame, Message as WreqWsMessage};

use crate::ai_serving::AiExecutionDecision;
use crate::execution_runtime::transport::{
    build_browser_wreq_client, build_request_headers, normalize_execution_proxy_url,
    ExecutionTransportControls,
};
use crate::frontdoor_loop_guard::gateway_frontdoor_self_loop_guard_error;
use crate::handlers::proxy::websocket::session::{
    WebSocketSessionLimits, RELAY_WRITE_TIMEOUT, TEARDOWN_WRITE_TIMEOUT,
};

#[derive(Clone, Copy)]
pub(crate) struct UpstreamWebSocketErrorCodes {
    pub(crate) upstream_url_missing: &'static str,
    pub(crate) upstream_url_invalid: &'static str,
    pub(crate) frontdoor_self_loop: &'static str,
    pub(crate) headers_invalid: &'static str,
    pub(crate) client_build_failed: &'static str,
    pub(crate) proxy_invalid: &'static str,
    pub(crate) tunnel_proxy_unsupported: &'static str,
    pub(crate) handshake_failed: &'static str,
    pub(crate) upgrade_rejected: &'static str,
    pub(crate) upgrade_failed: &'static str,
}

pub(crate) struct UpstreamWebSocketConnection {
    pub(crate) socket: UpstreamWebSocket,
    pub(crate) response_headers: BTreeMap<String, String>,
}

/// Type-erased IO used by the non-fingerprint upstream WebSocket.  TLS (both
/// direct and proxied) is layered inside before the stream is boxed, so one
/// concrete `WebSocketStream` type covers direct, HTTP-proxied, and
/// SOCKS-proxied connections.
pub(crate) trait UpstreamIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> UpstreamIo for T {}

pub(crate) type BoxedUpstreamIo = Box<dyn UpstreamIo>;

/// Upstream WebSocket transport.
///
/// Non-browser-profile relays use `tokio-tungstenite` (single 0.28 line
/// across the workspace).  The `Browser` variant is the only remaining user
/// of `wreq::ws`, and the sole reason `wreq` is kept at all: TLS browser
/// fingerprint impersonation (see `docs/adr/wreq-exit-strategy.md`).
pub(crate) enum UpstreamWebSocket {
    Plain(tokio_tungstenite::WebSocketStream<BoxedUpstreamIo>),
    Browser(wreq::ws::WebSocket),
}

/// Why an upstream frame could not be written or read.  Callers collapse this
/// to their own error codes; it only exists to unify the wreq and tungstenite
/// error types behind one sink/stream.
#[derive(Debug)]
pub(crate) struct UpstreamWebSocketError;

impl futures_util::Stream for UpstreamWebSocket {
    type Item = Result<UpstreamWsMessage, UpstreamWebSocketError>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.get_mut() {
            UpstreamWebSocket::Plain(socket) => Pin::new(socket).poll_next(cx).map(|item| {
                item.map(|message| message.map(Into::into).map_err(|_| UpstreamWebSocketError))
            }),
            UpstreamWebSocket::Browser(socket) => Pin::new(socket).poll_next(cx).map(|item| {
                item.map(|message| message.map(Into::into).map_err(|_| UpstreamWebSocketError))
            }),
        }
    }
}

impl futures_util::Sink<UpstreamWsMessage> for UpstreamWebSocket {
    type Error = UpstreamWebSocketError;

    fn poll_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            UpstreamWebSocket::Plain(socket) => Pin::new(socket)
                .poll_ready(cx)
                .map_err(|_| UpstreamWebSocketError),
            UpstreamWebSocket::Browser(socket) => Pin::new(socket)
                .poll_ready(cx)
                .map_err(|_| UpstreamWebSocketError),
        }
    }

    fn start_send(self: Pin<&mut Self>, message: UpstreamWsMessage) -> Result<(), Self::Error> {
        match self.get_mut() {
            UpstreamWebSocket::Plain(socket) => Pin::new(socket)
                .start_send(message.into_tungstenite())
                .map_err(|_| UpstreamWebSocketError),
            UpstreamWebSocket::Browser(socket) => Pin::new(socket)
                .start_send(message.into_wreq())
                .map_err(|_| UpstreamWebSocketError),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            UpstreamWebSocket::Plain(socket) => Pin::new(socket)
                .poll_flush(cx)
                .map_err(|_| UpstreamWebSocketError),
            UpstreamWebSocket::Browser(socket) => Pin::new(socket)
                .poll_flush(cx)
                .map_err(|_| UpstreamWebSocketError),
        }
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        match self.get_mut() {
            UpstreamWebSocket::Plain(socket) => Pin::new(socket)
                .poll_close(cx)
                .map_err(|_| UpstreamWebSocketError),
            UpstreamWebSocket::Browser(socket) => Pin::new(socket)
                .poll_close(cx)
                .map_err(|_| UpstreamWebSocketError),
        }
    }
}

/// A transport-neutral upstream WebSocket frame.  Sessions only ever see this
/// type; the per-client encoding (wreq vs tungstenite) stays at the sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpstreamWsMessage {
    Text(String),
    Binary(Bytes),
    Ping(Bytes),
    Pong(Bytes),
    Close(Option<UpstreamWsCloseFrame>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpstreamWsCloseFrame {
    pub(crate) code: u16,
    pub(crate) reason: String,
}

impl UpstreamWsMessage {
    pub(crate) fn text(string: impl Into<String>) -> Self {
        UpstreamWsMessage::Text(string.into())
    }
}

impl From<WreqWsMessage> for UpstreamWsMessage {
    fn from(message: WreqWsMessage) -> Self {
        match message {
            WreqWsMessage::Text(text) => UpstreamWsMessage::Text(text.to_string()),
            WreqWsMessage::Binary(data) => UpstreamWsMessage::Binary(data),
            WreqWsMessage::Ping(data) => UpstreamWsMessage::Ping(data),
            WreqWsMessage::Pong(data) => UpstreamWsMessage::Pong(data),
            WreqWsMessage::Close(frame) => {
                UpstreamWsMessage::Close(frame.map(|frame| UpstreamWsCloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.to_string(),
                }))
            }
        }
    }
}

impl UpstreamWsMessage {
    fn into_wreq(self) -> WreqWsMessage {
        match self {
            UpstreamWsMessage::Text(text) => WreqWsMessage::Text(text.into()),
            UpstreamWsMessage::Binary(data) => WreqWsMessage::Binary(data),
            UpstreamWsMessage::Ping(data) => WreqWsMessage::Ping(data),
            UpstreamWsMessage::Pong(data) => WreqWsMessage::Pong(data),
            UpstreamWsMessage::Close(frame) => {
                WreqWsMessage::Close(frame.map(|frame| WreqCloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.into(),
                }))
            }
        }
    }

    fn into_tungstenite(self) -> tokio_tungstenite::tungstenite::Message {
        use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
        match self {
            UpstreamWsMessage::Text(text) => TungsteniteMessage::text(text),
            UpstreamWsMessage::Binary(data) => TungsteniteMessage::Binary(data),
            UpstreamWsMessage::Ping(data) => TungsteniteMessage::Ping(data),
            UpstreamWsMessage::Pong(data) => TungsteniteMessage::Pong(data),
            UpstreamWsMessage::Close(frame) => TungsteniteMessage::Close(frame.map(|frame| {
                tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.into(),
                }
            })),
        }
    }
}

impl From<tokio_tungstenite::tungstenite::Message> for UpstreamWsMessage {
    fn from(message: tokio_tungstenite::tungstenite::Message) -> Self {
        use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
        match message {
            TungsteniteMessage::Text(text) => UpstreamWsMessage::Text(text.to_string()),
            TungsteniteMessage::Binary(data) => UpstreamWsMessage::Binary(data),
            TungsteniteMessage::Ping(data) => UpstreamWsMessage::Ping(data),
            TungsteniteMessage::Pong(data) => UpstreamWsMessage::Pong(data),
            TungsteniteMessage::Close(frame) => {
                UpstreamWsMessage::Close(frame.map(|frame| UpstreamWsCloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.to_string(),
                }))
            }
            // Raw frames are never surfaced by the stream API.
            TungsteniteMessage::Frame(_) => UpstreamWsMessage::Binary(Bytes::new()),
        }
    }
}

pub(crate) async fn connect_upstream_websocket(
    decision: &AiExecutionDecision,
    limits: WebSocketSessionLimits,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<UpstreamWebSocketConnection, &'static str> {
    let upstream_url = decision
        .upstream_url
        .as_deref()
        .ok_or(errors.upstream_url_missing)?;
    let upstream_url = guarded_websocket_upstream_url(
        upstream_url,
        errors.upstream_url_invalid,
        errors.frontdoor_self_loop,
    )?;
    let headers =
        websocket_handshake_headers(&decision.provider_request_headers, errors.headers_invalid)?;
    if decision.transport_profile.is_some() {
        return connect_browser_upstream_websocket(
            decision,
            &upstream_url,
            headers,
            limits,
            errors,
        )
        .await;
    }
    connect_plain_upstream_websocket(decision, &upstream_url, headers, limits, errors).await
}

/// Browser-profile (TLS fingerprint impersonation) upstream handshake.  This
/// is the ONLY remaining `wreq::ws` consumer in the gateway: impersonation
/// requires the wreq TLS stack, so the WS upgrade rides the same client.
async fn connect_browser_upstream_websocket(
    decision: &AiExecutionDecision,
    upstream_url: &Url,
    headers: HeaderMap,
    limits: WebSocketSessionLimits,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<UpstreamWebSocketConnection, &'static str> {
    let timeouts = websocket_timeouts(decision);
    let profile = decision
        .transport_profile
        .as_ref()
        .ok_or(errors.client_build_failed)?;
    let client = build_browser_wreq_client(
        timeouts.as_ref(),
        decision.proxy.as_ref(),
        profile,
        ExecutionTransportControls::default(),
        false,
    )
    .map_err(|_| errors.client_build_failed)?;
    let response = client
        .websocket(upstream_url.as_str())
        .headers(headers)
        .max_frame_size(limits.max_frame_size)
        .max_message_size(limits.max_message_size)
        .send()
        .await
        .map_err(|_| errors.handshake_failed)?;
    if response.status().as_u16() != 101 {
        return Err(errors.upgrade_rejected);
    }
    let response_headers = websocket_response_headers(response.headers());
    let socket = response
        .into_websocket()
        .await
        .map_err(|_| errors.upgrade_failed)?;
    Ok(UpstreamWebSocketConnection {
        socket: UpstreamWebSocket::Browser(socket),
        response_headers,
    })
}

/// Non-fingerprint upstream handshake on tokio-tungstenite.  Direct
/// connections reuse the DNS pinning and private/reserved IP checks from the
/// previous wreq client; proxied connections keep provider DNS remote (HTTP
/// absolute-form for `ws://`, CONNECT for `wss://`, SOCKS5h domains).
async fn connect_plain_upstream_websocket(
    decision: &AiExecutionDecision,
    upstream_url: &Url,
    headers: HeaderMap,
    limits: WebSocketSessionLimits,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<UpstreamWebSocketConnection, &'static str> {
    let connect_timeout = websocket_timeouts(decision)
        .and_then(|timeouts| timeouts.connect_ms)
        .map(Duration::from_millis);
    let ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_frame_size(Some(limits.max_frame_size))
        .max_message_size(Some(limits.max_message_size));

    let future = connect_plain_upstream(decision, upstream_url, &headers, ws_config, errors);
    let (socket, response_headers) = match connect_timeout {
        Some(timeout) => tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| errors.handshake_failed)??,
        None => future.await?,
    };
    Ok(UpstreamWebSocketConnection {
        socket,
        response_headers,
    })
}

/// Establishes the full non-fingerprint upstream WebSocket: direct TCP with
/// DNS pinning, or a proxy negotiation that keeps provider DNS remote, with
/// TLS applied last for `wss://` targets, followed by the upgrade handshake.
async fn connect_plain_upstream(
    decision: &AiExecutionDecision,
    upstream_url: &Url,
    headers: &HeaderMap,
    ws_config: tokio_tungstenite::tungstenite::protocol::WebSocketConfig,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<(UpstreamWebSocket, BTreeMap<String, String>), &'static str> {
    let proxy_url = resolve_websocket_proxy_url(decision.proxy.as_ref(), errors)?;
    let host = upstream_url.host_str().ok_or(errors.upstream_url_invalid)?;
    let port = upstream_url
        .port_or_known_default()
        .ok_or(errors.upstream_url_invalid)?;
    let is_tls = upstream_url.scheme() == "wss";

    let mut io: BoxedUpstreamIo = match proxy_url {
        Some(proxy_url) => {
            let proxy = ParsedUpstreamProxy::parse(&proxy_url).map_err(|_| errors.proxy_invalid)?;
            let mut stream = connect_proxy_tcp(&proxy, errors).await?;
            match proxy.scheme {
                UpstreamProxyScheme::Http => {
                    if is_tls {
                        http_connect(
                            &mut stream,
                            &format!("{host}:{port}"),
                            &proxy,
                            errors.handshake_failed,
                        )
                        .await?;
                    } else {
                        // Plain `ws://` through an HTTP proxy uses absolute-form
                        // forwarding (the request URI carries the full ws:// URL),
                        // matching the previous wreq client behavior.
                        let (socket, response_headers) = forward_websocket_over_http_proxy(
                            Box::new(stream),
                            upstream_url,
                            headers,
                            &proxy,
                            ws_config,
                            errors,
                        )
                        .await?;
                        return Ok((UpstreamWebSocket::Plain(socket), response_headers));
                    }
                }
                UpstreamProxyScheme::Socks5h => {
                    socks5_connect(&mut stream, &proxy, host, port, errors.handshake_failed)
                        .await?;
                }
            }
            if proxy.tls {
                Box::new(tls_wrap(Box::new(stream), &proxy.host, errors).await?)
            } else {
                Box::new(stream)
            }
        }
        None => connect_direct_upstream_io(upstream_url, errors).await?,
    };

    if is_tls {
        io = Box::new(tls_wrap(io, host, errors).await?);
    }

    let request =
        build_tungstenite_request(upstream_url, headers).map_err(|_| errors.headers_invalid)?;
    let (socket, response) =
        tokio_tungstenite::client_async_with_config(request, io, Some(ws_config))
            .await
            .map_err(|error| match error {
                // A completed HTTP response that is not a 101 Switching Protocols
                // means the upstream refused the upgrade, distinct from a
                // transport-level handshake failure.
                tokio_tungstenite::tungstenite::Error::Http(response)
                    if response.status().as_u16() != 101 =>
                {
                    errors.upgrade_rejected
                }
                _ => errors.handshake_failed,
            })?;
    let response_headers = websocket_response_headers(response.headers());
    Ok((UpstreamWebSocket::Plain(socket), response_headers))
}

/// Builds the tungstenite client handshake request for `url` with the
/// provider headers attached.
fn build_tungstenite_request(
    url: &Url,
    headers: &HeaderMap,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, ()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.as_str().into_client_request().map_err(|_| ())?;
    request.headers_mut().extend(headers.clone());
    Ok(request)
}

/// Resolves and pins a direct WebSocket target once, preserving the rebinding
/// boundary established by the previous wreq client, and refuses
/// private/reserved answers unless the target is loopback `ws://`.
async fn connect_direct_upstream_io(
    upstream_url: &Url,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<BoxedUpstreamIo, &'static str> {
    let host = upstream_url.host_str().ok_or(errors.upstream_url_invalid)?;
    let port = upstream_url
        .port_or_known_default()
        .ok_or(errors.upstream_url_invalid)?;
    let addresses = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        vec![std::net::SocketAddr::new(ip, port)]
    } else {
        aether_http::lookup_host_with_limits(host, port, aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT)
            .await
            .map_err(|_| errors.upstream_url_invalid)?
    };
    let allows_loopback = host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    let unsafe_answer = if allows_loopback {
        addresses.iter().any(|address| !address.ip().is_loopback())
    } else {
        addresses
            .iter()
            .any(|address| aether_http::is_private_or_reserved_ip(address.ip()))
    };
    if addresses.is_empty() || unsafe_answer {
        return Err(errors.upstream_url_invalid);
    }
    let mut last_error = io::Error::new(io::ErrorKind::AddrNotAvailable, "no address");
    for address in addresses {
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(Box::new(stream)),
            Err(error) => last_error = error,
        }
    }
    Err(match last_error.kind() {
        io::ErrorKind::TimedOut => errors.handshake_failed,
        _ => errors.upstream_url_invalid,
    })
}

fn plain_upstream_tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }))
}

/// Applies rustls (webpki roots, no client auth) over an established stream.
async fn tls_wrap<S>(
    stream: S,
    host: &str,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<tokio_rustls::client::TlsStream<S>, &'static str>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| errors.upstream_url_invalid)?;
    tokio_rustls::TlsConnector::from(plain_upstream_tls_config())
        .connect(server_name, stream)
        .await
        .map_err(|_| errors.handshake_failed)
}

/// Parsed upstream proxy origin (the proxy URL was already validated by
/// `resolve_websocket_proxy_url`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpstreamProxyScheme {
    Http,
    /// Provider DNS stays remote: the SOCKS handshake carries the domain.
    Socks5h,
}

#[derive(Debug, Clone)]
struct ParsedUpstreamProxy {
    scheme: UpstreamProxyScheme,
    /// TLS to the proxy itself (`https://` proxy URLs).
    tls: bool,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
}

impl ParsedUpstreamProxy {
    fn parse(raw: &str) -> Result<Self, ()> {
        let parsed = Url::parse(raw).map_err(|_| ())?;
        let (scheme, tls) = match parsed.scheme().to_ascii_lowercase().as_str() {
            "http" => (UpstreamProxyScheme::Http, false),
            "https" => (UpstreamProxyScheme::Http, true),
            "socks5" | "socks5h" => (UpstreamProxyScheme::Socks5h, false),
            _ => return Err(()),
        };
        let host = parsed.host_str().map(str::to_string).ok_or(())?;
        let port = parsed.port().unwrap_or(if tls {
            443
        } else if scheme == UpstreamProxyScheme::Http {
            80
        } else {
            1080
        });
        let username = (!parsed.username().is_empty()).then(|| parsed.username().to_string());
        let password = parsed.password().map(str::to_string);
        Ok(Self {
            scheme,
            tls,
            host,
            port,
            username,
            password,
        })
    }

    fn basic_auth_header(&self) -> Option<String> {
        let username = self.username.as_deref()?;
        let mut credentials = String::with_capacity(
            username.len() + self.password.as_ref().map(|value| value.len()).unwrap_or(0) + 1,
        );
        credentials.push_str(username);
        credentials.push(':');
        if let Some(password) = self.password.as_deref() {
            credentials.push_str(password);
        }
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials)
        ))
    }
}

/// TCP connect to the proxy, with local DNS resolution of the proxy host.
async fn connect_proxy_tcp(
    proxy: &ParsedUpstreamProxy,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<TcpStream, &'static str> {
    let addresses = aether_http::lookup_host_with_limits(
        &proxy.host,
        proxy.port,
        aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
    )
    .await
    .map_err(|_| errors.proxy_invalid)?;
    let mut last_error = io::Error::new(io::ErrorKind::AddrNotAvailable, "no address");
    for address in addresses {
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = error,
        }
    }
    Err(match last_error.kind() {
        io::ErrorKind::TimedOut => errors.handshake_failed,
        _ => errors.proxy_invalid,
    })
}

/// HTTP CONNECT tunneling for `wss://` through an HTTP(S) proxy.
async fn http_connect(
    stream: &mut TcpStream,
    target_authority: &str,
    proxy: &ParsedUpstreamProxy,
    error_code: &'static str,
) -> Result<(), &'static str> {
    let mut request = format!(
        "CONNECT {target_authority} HTTP/1.1\r\nHost: {target_authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if let Some(auth) = proxy.basic_auth_header() {
        request.push_str("Proxy-Authorization: ");
        request.push_str(&auth);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| error_code)?;
    stream.flush().await.map_err(|_| error_code)?;

    let mut response = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if response.len() >= 16 * 1024 {
            return Err(error_code);
        }
        let read = stream.read(&mut chunk).await.map_err(|_| error_code)?;
        if read == 0 {
            return Err(error_code);
        }
        response.extend_from_slice(&chunk[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let status_line_end = response
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or(error_code)?;
    let status_line = std::str::from_utf8(&response[..status_line_end]).map_err(|_| error_code)?;
    let status = status_line.split_whitespace().nth(1).unwrap_or_default();
    if status == "200" {
        Ok(())
    } else {
        Err(error_code)
    }
}

/// SOCKS5 connect with the provider hostname sent as a domain (remote DNS,
/// matching the gateway's normalized `socks5h` proxy URLs).
async fn socks5_connect(
    stream: &mut TcpStream,
    proxy: &ParsedUpstreamProxy,
    target_host: &str,
    target_port: u16,
    error_code: &'static str,
) -> Result<(), &'static str> {
    let requires_auth = proxy.username.is_some();
    if requires_auth {
        stream
            .write_all(&[0x05, 0x02, 0x00, 0x02])
            .await
            .map_err(|_| error_code)?;
    } else {
        stream
            .write_all(&[0x05, 0x01, 0x00])
            .await
            .map_err(|_| error_code)?;
    }

    let mut method_response = [0u8; 2];
    stream
        .read_exact(&mut method_response)
        .await
        .map_err(|_| error_code)?;
    if method_response[0] != 0x05 {
        return Err(error_code);
    }
    match method_response[1] {
        0x00 => {}
        0x02 => socks5_authenticate(stream, proxy, error_code).await?,
        _ => return Err(error_code),
    }

    let host = target_host.as_bytes();
    if host.len() > u8::MAX as usize {
        return Err(error_code);
    }
    let mut request = Vec::with_capacity(4 + 1 + host.len() + 2);
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host.len() as u8]);
    request.extend_from_slice(host);
    request.extend_from_slice(&target_port.to_be_bytes());
    stream.write_all(&request).await.map_err(|_| error_code)?;

    let mut response = [0u8; 4];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|_| error_code)?;
    if response[0] != 0x05 || response[1] != 0x00 {
        return Err(error_code);
    }
    match response[3] {
        0x01 => {
            let mut ignored = [0u8; 4 + 2];
            stream
                .read_exact(&mut ignored)
                .await
                .map_err(|_| error_code)?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await.map_err(|_| error_code)?;
            let mut ignored = vec![0u8; len[0] as usize + 2];
            stream
                .read_exact(&mut ignored)
                .await
                .map_err(|_| error_code)?;
        }
        0x04 => {
            let mut ignored = [0u8; 16 + 2];
            stream
                .read_exact(&mut ignored)
                .await
                .map_err(|_| error_code)?;
        }
        _ => return Err(error_code),
    }
    Ok(())
}

async fn socks5_authenticate(
    stream: &mut TcpStream,
    proxy: &ParsedUpstreamProxy,
    error_code: &'static str,
) -> Result<(), &'static str> {
    let username = proxy.username.as_deref().unwrap_or_default().as_bytes();
    let password = proxy.password.as_deref().unwrap_or_default().as_bytes();
    if username.len() > u8::MAX as usize || password.len() > u8::MAX as usize {
        return Err(error_code);
    }
    let mut request = Vec::with_capacity(3 + username.len() + password.len());
    request.extend_from_slice(&[0x01, username.len() as u8]);
    request.extend_from_slice(username);
    request.push(password.len() as u8);
    request.extend_from_slice(password);
    stream.write_all(&request).await.map_err(|_| error_code)?;
    let mut response = [0u8; 2];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|_| error_code)?;
    if response[1] != 0x00 {
        return Err(error_code);
    }
    Ok(())
}

/// Absolute-form WebSocket forwarding for plain `ws://` through an HTTP
/// proxy: the proxy receives `GET ws://host/path HTTP/1.1` and forwards it,
/// exactly like the previous wreq client.  Returns an already-upgraded client
/// WebSocket over the (possibly TLS-to-proxy) connection.
async fn forward_websocket_over_http_proxy(
    io: BoxedUpstreamIo,
    upstream_url: &Url,
    headers: &HeaderMap,
    proxy: &ParsedUpstreamProxy,
    ws_config: tokio_tungstenite::tungstenite::protocol::WebSocketConfig,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<BoxedUpstreamIo>,
        BTreeMap<String, String>,
    ),
    &'static str,
> {
    let key = tokio_tungstenite::tungstenite::handshake::client::generate_key();
    let host = upstream_url.host_str().ok_or(errors.upstream_url_invalid)?;
    let authority = match upstream_url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let path = upstream_url
        .path()
        .trim_start_matches('/')
        .trim_end_matches('/');
    let mut request = format!(
        "GET ws://{authority}/{path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
    );
    for (name, value) in headers {
        if name == HOST || name == CONNECTION || name == UPGRADE {
            continue;
        }
        let value = value.to_str().map_err(|_| errors.headers_invalid)?;
        request.push_str(name.as_str());
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    if let Some(auth) = proxy.basic_auth_header() {
        request.push_str("Proxy-Authorization: ");
        request.push_str(&auth);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");

    let mut io = io;
    io.write_all(request.as_bytes())
        .await
        .map_err(|_| errors.handshake_failed)?;

    let mut response = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if response.len() >= 64 * 1024 {
            return Err(errors.handshake_failed);
        }
        let read = io
            .read(&mut chunk)
            .await
            .map_err(|_| errors.handshake_failed)?;
        if read == 0 {
            return Err(errors.handshake_failed);
        }
        response.extend_from_slice(&chunk[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .ok_or(errors.handshake_failed)?;
    let head = String::from_utf8_lossy(&response[..header_end]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default();
    if status != "101" {
        return Err(errors.upgrade_rejected);
    }
    let response_headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
        .collect::<BTreeMap<_, _>>();
    let retained = websocket_response_headers_owned(response_headers);

    // The header reader may have consumed bytes of the first WebSocket frame
    // along with the response block; replay the buffered remainder in front of
    // the socket before adopting it as an upgraded client WebSocket.
    let leftover = response.split_off(header_end);
    let socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
        Box::new(PrefixedIo::new(leftover, io)) as BoxedUpstreamIo,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        Some(ws_config),
    )
    .await;
    Ok((socket, retained))
}

/// Replays bytes that were read past the end of the proxy's handshake
/// response before delegating to the underlying stream.
struct PrefixedIo {
    buffered: std::io::Cursor<Vec<u8>>,
    inner: BoxedUpstreamIo,
}

impl PrefixedIo {
    fn new(buffered: Vec<u8>, inner: BoxedUpstreamIo) -> Self {
        Self {
            buffered: std::io::Cursor::new(buffered),
            inner,
        }
    }
}

impl AsyncRead for PrefixedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let position = self.buffered.position() as usize;
        let buffered_len = self.buffered.get_ref().len();
        if position < buffered_len {
            let len = (buffered_len - position).min(buf.remaining());
            let chunk = self.buffered.get_ref()[position..position + len].to_vec();
            buf.put_slice(&chunk);
            self.buffered.set_position((position + len) as u64);
            return std::task::Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PrefixedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn websocket_response_headers_owned(headers: BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .into_iter()
        .filter(|(name, _)| !websocket_response_header_name_is_credential_bearing(name))
        .filter_map(|(name, value)| {
            let normalized = name.to_ascii_lowercase();
            if crate::headers::should_skip_response_header(&normalized) {
                return None;
            }
            Some((normalized, value))
        })
        .collect()
}

fn websocket_response_header_name_is_credential_bearing(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "proxy-authorization"
            | "www-authenticate"
            | "proxy-authenticate"
            | "authentication-info"
            | "proxy-authentication-info"
            | "cookie"
            | "set-cookie"
            | "set-cookie2"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
    )
}

fn guarded_websocket_upstream_url(
    raw: &str,
    invalid_code: &'static str,
    frontdoor_self_loop_code: &'static str,
) -> Result<Url, &'static str> {
    let upstream_url = websocket_upstream_url(raw, invalid_code)?;
    if gateway_frontdoor_self_loop_guard_error(upstream_url.as_str()).is_some() {
        return Err(frontdoor_self_loop_code);
    }
    Ok(upstream_url)
}

fn websocket_response_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    let connection_declared = aether_http::connection_declared_header_names(
        headers
            .get_all(http::header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok()),
    );
    headers
        .iter()
        .filter(|(name, _)| websocket_response_header_is_safe_to_retain(name))
        .filter_map(|(name, value)| {
            let normalized = name.as_str().to_ascii_lowercase();
            if crate::headers::should_skip_response_header(&normalized)
                || connection_declared.contains(&normalized)
            {
                return None;
            }
            value
                .to_str()
                .ok()
                .map(|value| (normalized, value.to_string()))
        })
        .collect()
}

fn websocket_response_header_is_safe_to_retain(name: &HeaderName) -> bool {
    !matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "www-authenticate"
            | "proxy-authenticate"
            | "authentication-info"
            | "proxy-authentication-info"
            | "cookie"
            | "set-cookie"
            | "set-cookie2"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
    )
}

pub(crate) fn websocket_upstream_url(
    raw: &str,
    invalid_code: &'static str,
) -> Result<Url, &'static str> {
    let mut url = Url::parse(raw).map_err(|_| invalid_code)?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_code);
    }
    let websocket_scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        _ => return Err(invalid_code),
    };
    url.set_scheme(websocket_scheme).map_err(|_| invalid_code)?;
    let literal_ip = match url.host() {
        Some(url::Host::Ipv4(address)) => Some(std::net::IpAddr::V4(address)),
        Some(url::Host::Ipv6(address)) => Some(std::net::IpAddr::V6(address)),
        _ => None,
    };
    if literal_ip.is_some_and(|address| {
        aether_http::is_private_or_reserved_ip(address)
            && !(url.scheme() == "ws" && address.is_loopback())
    }) {
        return Err(invalid_code);
    }
    Ok(url)
}

pub(crate) fn websocket_handshake_headers(
    provider_headers: &BTreeMap<String, String>,
    invalid_code: &'static str,
) -> Result<HeaderMap, &'static str> {
    // `build_request_headers` already strips `Connection` itself. Read the
    // dynamic hop-by-hop names from the source map first, otherwise a header
    // named by `Connection: keep-alive, x-provider-hop` would survive.
    let connection_scoped_names = provider_headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(CONNECTION.as_str()))
        .flat_map(|(_, value)| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect::<Vec<_>>();
    let mut headers =
        build_request_headers(provider_headers, None, false).map_err(|_| invalid_code)?;
    for name in connection_scoped_names {
        headers.remove(name);
    }
    for header in [
        ACCEPT,
        ACCEPT_ENCODING,
        CONNECTION,
        CONTENT_ENCODING,
        CONTENT_LENGTH,
        CONTENT_TYPE,
        HOST,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(header);
    }
    for header in ["keep-alive", "proxy-connection"] {
        headers.remove(header);
    }
    // The WebSocket client owns every Sec-WebSocket-* field, including
    // extensions introduced after this gateway was built.  Passing a
    // downstream handshake field through here can corrupt negotiation or
    // disclose the client's nonce/subprotocol to a different upstream.
    let websocket_managed_names = headers
        .keys()
        .filter(|name| name.as_str().starts_with("sec-websocket-"))
        .cloned()
        .collect::<Vec<_>>();
    for name in websocket_managed_names {
        headers.remove(name);
    }
    Ok(headers)
}

fn resolve_websocket_proxy_url(
    proxy: Option<&ProxySnapshot>,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<Option<String>, &'static str> {
    let Some(proxy) = proxy else {
        return Ok(None);
    };
    if proxy.enabled == Some(false) {
        return Ok(None);
    }
    if let Some(proxy_url) = proxy
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let parsed = Url::parse(proxy_url).map_err(|_| errors.proxy_invalid)?;
        if !matches!(
            parsed.scheme().to_ascii_lowercase().as_str(),
            "http" | "https" | "socks5" | "socks5h"
        ) || parsed.host_str().is_none()
            || !matches!(parsed.path(), "" | "/")
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(errors.proxy_invalid);
        }
        // Manual proxy nodes bind credentials to the node identity before a
        // snapshot reaches this path. Reject userinfo on an otherwise
        // unbound snapshot so an arbitrary decision cannot smuggle proxy
        // credentials through a URL; preserve the established node-auth URL
        // form for authenticated manual proxy nodes.
        if (!parsed.username().is_empty() || parsed.password().is_some()) && proxy.node_id.is_none()
        {
            return Err(errors.proxy_invalid);
        }
        let normalized =
            normalize_execution_proxy_url(proxy_url).map_err(|_| errors.proxy_invalid)?;
        return Ok(Some(normalized));
    }
    if proxy.node_id.is_some() || proxy.mode.as_deref() == Some("tunnel") {
        return Err(errors.tunnel_proxy_unsupported);
    }
    Err(errors.proxy_invalid)
}

pub(crate) fn websocket_timeouts(
    decision: &AiExecutionDecision,
) -> Option<aether_contracts::ExecutionTimeouts> {
    let mut timeouts = decision.timeouts.clone()?;
    timeouts.read_ms = None;
    timeouts.first_byte_ms = None;
    timeouts.total_ms = None;
    Some(timeouts)
}

/// Why a frame did not reach its peer.  A timeout is reported separately from
/// a socket error because the two describe different peers: one has gone away,
/// the other is still connected but has stopped reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSocketWriteError {
    Failed,
    TimedOut,
    Cancelled,
}

impl WebSocketWriteError {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "write_failed",
            Self::TimedOut => "write_timeout",
            Self::Cancelled => "write_cancelled",
        }
    }
}

/// A small per-direction buffer keeps a slow reader from blocking the opposite
/// WebSocket direction while still applying bounded backpressure. At the Live
/// audio cadence this is deliberately only a short burst buffer, not a place
/// where a session can accumulate unbounded media.
pub(crate) const RELAY_FRAME_QUEUE_CAPACITY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSocketRelayQueueError {
    Closed,
    Cancelled,
}

/// Shared cancellation for both read/write halves of a bidirectional relay.
///
/// Queue admission and socket writes both observe this token, so a connection
/// deadline or lease loss can interrupt a full queue and an in-flight slow
/// write immediately instead of waiting for [`RELAY_WRITE_TIMEOUT`].
#[derive(Clone, Default)]
pub(crate) struct WebSocketRelayPumpControl {
    cancellation: CancellationToken,
}

impl WebSocketRelayPumpControl {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }

    pub(crate) async fn enqueue<T>(
        &self,
        sender: &mpsc::Sender<T>,
        message: T,
    ) -> Result<(), WebSocketRelayQueueError> {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(WebSocketRelayQueueError::Cancelled),
            result = sender.send(message) => {
                result.map_err(|_| WebSocketRelayQueueError::Closed)
            }
        }
    }

    pub(crate) async fn send<F>(&self, write: F) -> Result<(), WebSocketWriteError>
    where
        F: std::future::Future<Output = Result<(), ()>>,
    {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(WebSocketWriteError::Cancelled),
            result = bounded_send(RELAY_WRITE_TIMEOUT, write) => result,
        }
    }
}

pub(crate) fn websocket_relay_frame_queue<T>() -> (mpsc::Sender<T>, mpsc::Receiver<T>) {
    mpsc::channel(RELAY_FRAME_QUEUE_CAPACITY)
}

/// Relays one frame to the client under [`RELAY_WRITE_TIMEOUT`].
pub(crate) async fn send_client_message(
    client_socket: &mut WebSocket,
    message: AxumWsMessage,
) -> Result<(), WebSocketWriteError> {
    bounded_send(
        RELAY_WRITE_TIMEOUT,
        client_socket.send(message).map_err(|_| ()),
    )
    .await
}

/// Sends one frame to the upstream under [`RELAY_WRITE_TIMEOUT`].
pub(crate) async fn send_upstream_message(
    upstream: &mut UpstreamWebSocket,
    message: UpstreamWsMessage,
) -> Result<(), WebSocketWriteError> {
    bounded_send(RELAY_WRITE_TIMEOUT, upstream.send(message).map_err(|_| ())).await
}

/// Queues one frame in the upstream sink without flushing it. Completion means
/// `start_send` succeeded, so callers must conservatively treat the frame as
/// possibly delivered even when a later flush fails or is cancelled.
pub(crate) async fn feed_upstream_message(
    upstream: &mut UpstreamWebSocket,
    message: UpstreamWsMessage,
) -> Result<(), WebSocketWriteError> {
    bounded_send(RELAY_WRITE_TIMEOUT, upstream.feed(message).map_err(|_| ())).await
}

/// Flushes frames previously queued with [`feed_upstream_message`].
pub(crate) async fn flush_upstream_messages(
    upstream: &mut UpstreamWebSocket,
) -> Result<(), WebSocketWriteError> {
    bounded_send(RELAY_WRITE_TIMEOUT, upstream.flush().map_err(|_| ())).await
}

/// Best-effort teardown write.  The caller is already ending the session, so
/// the outcome only matters for keeping the wait bounded.
async fn send_teardown_message<F>(write: F)
where
    F: std::future::Future<Output = Result<(), ()>>,
{
    let _ = bounded_send(TEARDOWN_WRITE_TIMEOUT, write).await;
}

async fn bounded_send<F>(budget: Duration, write: F) -> Result<(), WebSocketWriteError>
where
    F: std::future::Future<Output = Result<(), ()>>,
{
    match tokio::time::timeout(budget, write).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(())) => Err(WebSocketWriteError::Failed),
        Err(_) => Err(WebSocketWriteError::TimedOut),
    }
}

/// Sends a WebSocket Close frame upstream without waiting on an unresponsive
/// provider.  The socket is dropped by the caller either way.
pub(crate) async fn close_upstream_socket(
    upstream: &mut UpstreamWebSocket,
    frame: Option<UpstreamWsCloseFrame>,
) {
    send_teardown_message(
        upstream
            .send(UpstreamWsMessage::Close(frame))
            .map_err(|_| ()),
    )
    .await;
}

pub(crate) fn upstream_message_to_client(message: UpstreamWsMessage) -> AxumWsMessage {
    match message {
        UpstreamWsMessage::Text(text) => AxumWsMessage::Text(text.into()),
        UpstreamWsMessage::Binary(data) => AxumWsMessage::Binary(data),
        UpstreamWsMessage::Ping(data) => AxumWsMessage::Ping(data),
        UpstreamWsMessage::Pong(data) => AxumWsMessage::Pong(data),
        UpstreamWsMessage::Close(frame) => {
            AxumWsMessage::Close(frame.map(|frame| AxumCloseFrame {
                code: frame.code,
                reason: frame.reason.into(),
            }))
        }
    }
}

pub(crate) fn client_message_to_upstream(message: AxumWsMessage) -> UpstreamWsMessage {
    match message {
        AxumWsMessage::Text(text) => UpstreamWsMessage::Text(text.to_string()),
        AxumWsMessage::Binary(data) => UpstreamWsMessage::Binary(data),
        AxumWsMessage::Ping(data) => UpstreamWsMessage::Ping(data),
        AxumWsMessage::Pong(data) => UpstreamWsMessage::Pong(data),
        AxumWsMessage::Close(frame) => {
            UpstreamWsMessage::Close(frame.map(|frame| UpstreamWsCloseFrame {
                code: frame.code,
                reason: frame.reason.to_string(),
            }))
        }
    }
}

/// Builds a Responses WebSocket error event in the shape understood by the
/// official client implementations.  The status is part of the event body,
/// not the WebSocket handshake, because the connection is already upgraded.
pub(crate) fn responses_websocket_error_event(
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
) -> serde_json::Value {
    responses_websocket_error_event_with_stream_id(status, error_type, code, message, None)
}

/// Builds a request-scoped Responses error. Callers must supply `stream_id`
/// only after validating the protocol's named-lane grammar; untrusted or
/// malformed identifiers must never be reflected into a provider event.
pub(crate) fn responses_websocket_error_event_with_stream_id(
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) -> serde_json::Value {
    let mut event = json!({
        "type": "error",
        "status": status,
        "error": {
            "type": error_type,
            "code": code,
            "message": message,
        },
    });
    if let Some(stream_id) = stream_id {
        event
            .as_object_mut()
            .expect("Responses error events are JSON objects")
            .insert(
                "stream_id".to_string(),
                serde_json::Value::String(stream_id.to_string()),
            );
    }
    event
}

pub(crate) async fn send_responses_websocket_error(
    client_socket: &mut WebSocket,
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
) {
    send_responses_websocket_error_with_stream_id(
        client_socket,
        status,
        error_type,
        code,
        message,
        None,
    )
    .await;
}

/// Sends a standard invalid-request error with a bounded, server-owned
/// parameter name. This is used for protocol fields such as
/// `previous_response_id`; no untrusted value is reflected.
pub(crate) async fn send_responses_websocket_error_with_param(
    client_socket: &mut WebSocket,
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
    param: &'static str,
) {
    let mut event = responses_websocket_error_event(status, error_type, code, message);
    event["error"]["param"] = serde_json::Value::String(param.to_string());
    send_teardown_message(
        client_socket
            .send(AxumWsMessage::Text(event.to_string().into()))
            .map_err(|_| ()),
    )
    .await;
}

pub(crate) async fn send_responses_websocket_error_with_stream_id(
    client_socket: &mut WebSocket,
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) {
    let event = responses_websocket_error_event_with_stream_id(
        status, error_type, code, message, stream_id,
    );
    send_teardown_message(
        client_socket
            .send(AxumWsMessage::Text(event.to_string().into()))
            .map_err(|_| ()),
    )
    .await;
}

pub(crate) async fn send_gateway_error(client_socket: &mut WebSocket, code: &str, message: &str) {
    send_gateway_error_with_status(client_socket, 400, code, message).await;
}

pub(crate) async fn send_gateway_error_with_stream_id(
    client_socket: &mut WebSocket,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) {
    send_gateway_error_with_status_and_stream_id(client_socket, 400, code, message, stream_id)
        .await;
}

pub(crate) async fn send_gateway_error_with_status(
    client_socket: &mut WebSocket,
    status: u16,
    code: &str,
    message: &str,
) {
    send_gateway_error_with_status_and_stream_id(client_socket, status, code, message, None).await;
}

pub(crate) async fn send_gateway_error_with_status_and_stream_id(
    client_socket: &mut WebSocket,
    status: u16,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) {
    send_responses_websocket_error_with_stream_id(
        client_socket,
        status,
        "gateway_error",
        code,
        message,
        stream_id,
    )
    .await;
}

pub(crate) async fn close_client_socket(client_socket: &mut WebSocket, code: u16, reason: &str) {
    send_teardown_message(
        client_socket
            .send(AxumWsMessage::Close(Some(AxumCloseFrame {
                code,
                reason: reason.to_string().into(),
            })))
            .map_err(|_| ()),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_send, guarded_websocket_upstream_url, resolve_websocket_proxy_url,
        responses_websocket_error_event, responses_websocket_error_event_with_stream_id,
        websocket_handshake_headers, websocket_relay_frame_queue, websocket_response_headers,
        websocket_upstream_url, ParsedUpstreamProxy, UpstreamWebSocketErrorCodes,
        WebSocketRelayPumpControl, WebSocketRelayQueueError, WebSocketWriteError,
        RELAY_FRAME_QUEUE_CAPACITY, RELAY_WRITE_TIMEOUT, TEARDOWN_WRITE_TIMEOUT,
    };
    use crate::ai_serving::AiExecutionDecision;
    use crate::frontdoor_loop_guard::configured_gateway_frontdoor_base_url;
    use aether_contracts::{ProxySnapshot, ResolvedTransportProfile};
    use axum::http::HeaderMap;
    use std::collections::BTreeMap;
    use std::time::Duration;
    use url::Url;

    #[tokio::test]
    async fn a_peer_that_never_drains_its_window_times_out_instead_of_pinning_the_relay() {
        let stalled = std::future::pending::<Result<(), ()>>();

        let outcome = bounded_send(Duration::from_millis(1), stalled).await;

        assert_eq!(outcome, Err(WebSocketWriteError::TimedOut));
    }

    #[tokio::test]
    async fn a_socket_error_is_reported_separately_from_a_stalled_peer() {
        let outcome = bounded_send(RELAY_WRITE_TIMEOUT, std::future::ready(Err(()))).await;

        assert_eq!(outcome, Err(WebSocketWriteError::Failed));
        assert_eq!(WebSocketWriteError::Failed.as_str(), "write_failed");
        assert_eq!(WebSocketWriteError::TimedOut.as_str(), "write_timeout");
        assert_eq!(WebSocketWriteError::Cancelled.as_str(), "write_cancelled");
    }

    #[tokio::test]
    async fn relay_frame_queue_is_bounded_and_fifo() {
        let (sender, mut receiver) = websocket_relay_frame_queue();
        for frame in 0..RELAY_FRAME_QUEUE_CAPACITY {
            sender
                .try_send(frame)
                .expect("the configured burst buffer should accept this frame");
        }
        assert!(matches!(
            sender.try_send(RELAY_FRAME_QUEUE_CAPACITY),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
        for expected in 0..RELAY_FRAME_QUEUE_CAPACITY {
            assert_eq!(receiver.recv().await, Some(expected));
        }
    }

    #[tokio::test]
    async fn relay_cancellation_interrupts_a_full_queue_without_waiting_for_capacity() {
        let control = WebSocketRelayPumpControl::new();
        let (sender, _receiver) = websocket_relay_frame_queue();
        for frame in 0..RELAY_FRAME_QUEUE_CAPACITY {
            sender.try_send(frame).expect("queue should fill exactly");
        }
        let enqueue = control.enqueue(&sender, RELAY_FRAME_QUEUE_CAPACITY);
        tokio::pin!(enqueue);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut enqueue)
            .await
            .is_err());

        control.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), enqueue)
                .await
                .expect("cancellation should wake a blocked producer"),
            Err(WebSocketRelayQueueError::Cancelled)
        );
    }

    #[tokio::test]
    async fn relay_cancellation_interrupts_a_stalled_socket_write() {
        let control = WebSocketRelayPumpControl::new();
        let write = control.send(std::future::pending::<Result<(), ()>>());
        tokio::pin!(write);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut write)
            .await
            .is_err());

        control.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), write)
                .await
                .expect("cancellation should wake a stalled writer"),
            Err(WebSocketWriteError::Cancelled)
        );
    }

    #[tokio::test]
    async fn a_write_that_completes_within_its_budget_succeeds() {
        let outcome = bounded_send(RELAY_WRITE_TIMEOUT, std::future::ready(Ok::<(), ()>(()))).await;

        assert_eq!(outcome, Ok(()));
    }

    #[test]
    fn teardown_writes_are_given_a_shorter_budget_than_relayed_frames() {
        assert!(TEARDOWN_WRITE_TIMEOUT < RELAY_WRITE_TIMEOUT);
    }

    #[test]
    fn upstream_handshake_observability_drops_credential_bearing_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-primary-used-percent", "10".parse().unwrap());
        headers.insert("x-request-id", "request-123".parse().unwrap());
        headers.insert("set-cookie", "session=secret".parse().unwrap());
        headers.insert("www-authenticate", "Bearer secret".parse().unwrap());
        headers.insert("authentication-info", "nextnonce=secret".parse().unwrap());

        let retained = websocket_response_headers(&headers);

        assert_eq!(
            retained
                .get("x-codex-primary-used-percent")
                .map(String::as_str),
            Some("10")
        );
        assert_eq!(
            retained.get("x-request-id").map(String::as_str),
            Some("request-123")
        );
        assert!(!retained.contains_key("set-cookie"));
        assert!(!retained.contains_key("www-authenticate"));
        assert!(!retained.contains_key("authentication-info"));
    }

    #[test]
    fn builds_a_client_compatible_responses_error_event() {
        let event = responses_websocket_error_event(
            400,
            "invalid_request_error",
            "previous_response_not_found",
            "Previous response was not found.",
        );

        assert_eq!(event["type"], "error");
        assert_eq!(event["status"], 400);
        assert_eq!(event["error"]["type"], "invalid_request_error");
        assert_eq!(event["error"]["code"], "previous_response_not_found");
        assert_eq!(
            event["error"]["message"],
            "Previous response was not found."
        );
        assert!(event.get("stream_id").is_none());
    }

    #[test]
    fn request_scoped_responses_errors_include_the_validated_named_stream() {
        let event = responses_websocket_error_event_with_stream_id(
            400,
            "gateway_error",
            "responses_websocket_named_stream_unsupported",
            "Named streams are not supported.",
            Some("main-lane_1.test"),
        );

        assert_eq!(event["stream_id"], "main-lane_1.test");
        assert_eq!(
            event["error"]["code"],
            "responses_websocket_named_stream_unsupported"
        );
    }

    #[test]
    fn maps_http_url_to_websocket_url_without_losing_path_or_query() {
        for (http_scheme, websocket_scheme) in [("https", "wss"), ("http", "ws")] {
            let url = websocket_upstream_url(
                &format!("{http_scheme}://example.test:8080/backend-api/codex/responses?x=1"),
                "invalid",
            )
            .expect("URL should be converted");
            assert_eq!(
                url.as_str(),
                format!("{websocket_scheme}://example.test:8080/backend-api/codex/responses?x=1")
            );
        }
    }

    #[test]
    fn rejects_upstream_url_with_credentials() {
        assert!(websocket_upstream_url("https://token@example.test/responses", "invalid").is_err());
    }

    #[test]
    fn websocket_upstream_url_accepts_ws_and_wss_with_safe_targets() {
        for allowed in [
            "wss://example.test/v1/responses",
            "https://example.test/v1/responses",
            "ws://example.test:8080/v1/responses",
            "http://example.test:8080/v1/responses",
            "http://8.8.8.8:8080/v1/responses",
            "wss://8.8.8.8/v1/responses",
            "ws://[2606:4700:4700::1111]:8080/v1/responses",
            "wss://[2606:4700:4700::1111]/v1/responses",
            "ws://localhost:8080/v1/responses",
            "http://127.42.0.1:8080/v1/responses",
            "ws://[::1]:8080/v1/responses",
        ] {
            assert!(
                websocket_upstream_url(allowed, "invalid").is_ok(),
                "{allowed}"
            );
        }
        for rejected in [
            "http://10.0.0.1/v1/responses",
            "wss://10.0.0.1/v1/responses",
            "wss://127.0.0.1/v1/responses",
            "wss://[::1]/v1/responses",
            "wss://[fd00::1]/v1/responses",
            "wss://[::ffff:127.0.0.1]/v1/responses",
            "wss://169.254.169.254/v1/responses",
            "wss://198.18.78.41/v1/responses",
            "wss://198.19.1.2/v1/responses",
            "ws://0.0.0.0:8080/v1/responses",
            "ws://[::ffff:127.0.0.1]:8080/v1/responses",
            "wss://example.test/v1/responses#secret",
            "ws://example.test/v1/responses#secret",
            "http://token@example.test/v1/responses",
            "ws://token@example.test/v1/responses",
            "ftp://example.test/v1/responses",
        ] {
            assert!(
                websocket_upstream_url(rejected, "invalid").is_err(),
                "{rejected}"
            );
        }
    }

    #[tokio::test]
    async fn websocket_client_build_defers_provider_dns_for_proxied_transport_profiles() {
        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };
        // Client construction must not resolve the provider or proxy hostnames:
        // proxied transports keep provider DNS remote and proxy DNS happens at
        // connect time inside the timeout budget.
        for url in ["http://proxy.invalid:8080", "socks5h://proxy.invalid:1080"] {
            let proxy = ProxySnapshot {
                enabled: Some(true),
                url: Some(url.to_string()),
                ..ProxySnapshot::default()
            };
            let normalized =
                resolve_websocket_proxy_url(Some(&proxy), errors).expect("proxy URL should parse");
            let parsed = ParsedUpstreamProxy::parse(normalized.as_deref().unwrap())
                .expect("normalized proxy URL should parse without DNS");
            assert_eq!(parsed.host, "proxy.invalid");
        }
    }

    #[test]
    fn active_websocket_proxy_without_a_target_fails_closed() {
        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };
        let missing = ProxySnapshot {
            enabled: Some(true),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&missing), errors),
            Err("proxy_invalid")
        );

        let tunnel = ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".to_string()),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&tunnel), errors),
            Err("tunnel_unsupported")
        );
    }

    #[tokio::test]
    async fn websocket_handshake_keeps_provider_dns_remote_for_http_and_socks_proxies() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };
        for profile in [
            None,
            Some(ResolvedTransportProfile {
                profile_id: "chrome136".to_string(),
                backend: aether_contracts::TRANSPORT_BACKEND_BROWSER_WREQ.to_string(),
                ..Default::default()
            }),
        ] {
            for scheme in ["http", "socks5", "socks5h"] {
                let listener = crate::test_support::bind_loopback_listener().await.unwrap();
                let proxy_addr = listener.local_addr().unwrap();
                let (release, released) = tokio::sync::oneshot::channel::<()>();
                let server = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    if scheme != "http" {
                        let mut greeting = [0; 2];
                        stream.read_exact(&mut greeting).await.unwrap();
                        assert_eq!(greeting[0], 5);
                        let mut methods = vec![0; greeting[1] as usize];
                        stream.read_exact(&mut methods).await.unwrap();
                        assert!(methods.contains(&0));
                        stream.write_all(&[5, 0]).await.unwrap();

                        let mut request = [0; 4];
                        stream.read_exact(&mut request).await.unwrap();
                        assert_eq!(
                            request,
                            [5, 1, 0, 3],
                            "proxy must receive a domain, not an IP"
                        );
                        let host_len = stream.read_u8().await.unwrap();
                        let mut host = vec![0; host_len as usize];
                        stream.read_exact(&mut host).await.unwrap();
                        assert_eq!(host, b"provider-dns.invalid");
                        assert_eq!(stream.read_u16().await.unwrap(), 80);
                        stream
                            .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                            .await
                            .unwrap();
                    }
                    let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let _ = released.await;
                    drop(socket);
                });
                let mut decision: AiExecutionDecision = serde_json::from_value(serde_json::json!({
                    "action": "proxy",
                    "upstream_url": "ws://provider-dns.invalid/v1/responses",
                    "proxy": {"enabled": true, "url": format!("{scheme}://{proxy_addr}")}
                }))
                .unwrap();
                decision.transport_profile = profile.clone();
                let connection = tokio::time::timeout(
                    Duration::from_secs(5),
                    super::connect_upstream_websocket(
                        &decision,
                        crate::handlers::proxy::websocket::session::RESPONSES_WEBSOCKET_SESSION_LIMITS,
                        errors,
                    ),
                )
                .await
                .expect("proxied handshake must not wait for local provider DNS")
                .unwrap_or_else(|error| panic!("{scheme} handshake failed: {error}"));
                release.send(()).unwrap();
                tokio::time::timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap();
                drop(connection);
            }
        }
    }

    #[test]
    fn rejects_responses_websocket_frontdoor_self_loop_before_connecting() {
        let base_url = configured_gateway_frontdoor_base_url();
        let raw_url = format!("{base_url}/v1/responses");

        assert_eq!(
            guarded_websocket_upstream_url(
                raw_url.as_str(),
                "responses_upstream_url_invalid",
                "responses_websocket_frontdoor_self_loop",
            ),
            Err("responses_websocket_frontdoor_self_loop")
        );
    }

    #[test]
    fn websocket_proxy_url_must_be_an_allowed_origin() {
        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };

        for value in [
            "file:///tmp/proxy",
            "http://proxy.example:8080/path",
            "http://proxy.example:8080?token=secret",
            "http://proxy.example:8080#fragment",
            "http://alice:password@proxy.example:8080",
        ] {
            let proxy = ProxySnapshot {
                enabled: Some(true),
                url: Some(value.to_string()),
                ..ProxySnapshot::default()
            };
            assert_eq!(
                resolve_websocket_proxy_url(Some(&proxy), errors),
                Err("proxy_invalid"),
                "proxy URL should be rejected: {value}"
            );
        }

        let authenticated_node = ProxySnapshot {
            enabled: Some(true),
            node_id: Some("manual-node-1".to_string()),
            url: Some("http://alice:password@proxy.example:8080".to_string()),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&authenticated_node), errors),
            Ok(Some(
                "http://alice:password@proxy.example:8080/".to_string()
            ))
        );

        let socks = ProxySnapshot {
            enabled: Some(true),
            url: Some("socks5://proxy.example:1080".to_string()),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&socks), errors),
            Ok(Some("socks5h://proxy.example:1080".to_string()))
        );
    }

    #[test]
    fn rejects_live_direct_and_sideband_frontdoor_self_loops_before_connecting() {
        let base_url = configured_gateway_frontdoor_base_url();

        for path in ["/v1/live", "/v1/live/rtc_test"] {
            let raw_url = format!("{base_url}{path}");
            assert_eq!(
                guarded_websocket_upstream_url(
                    raw_url.as_str(),
                    "codex_live_upstream_url_invalid",
                    "codex_live_websocket_frontdoor_self_loop",
                ),
                Err("codex_live_websocket_frontdoor_self_loop"),
                "{path} must be rejected before an upstream handshake"
            );
        }
    }

    #[test]
    fn upstream_handshake_keeps_provider_auth_but_drops_transport_managed_headers() {
        let provider_headers = BTreeMap::from([
            (
                "authorization".to_string(),
                "Bearer provider-token".to_string(),
            ),
            ("x-api-key".to_string(), "provider-api-key".to_string()),
            (
                "cookie".to_string(),
                "provider_session=provider-cookie".to_string(),
            ),
            (
                "connection".to_string(),
                "keep-alive, x-provider-hop".to_string(),
            ),
            ("x-provider-hop".to_string(), "must-not-pass".to_string()),
            ("upgrade".to_string(), "websocket".to_string()),
            (
                "sec-websocket-key".to_string(),
                "downstream-nonce".to_string(),
            ),
            (
                "sec-websocket-future-field".to_string(),
                "future-value".to_string(),
            ),
            (
                "proxy-authorization".to_string(),
                "Basic must-not-pass".to_string(),
            ),
            ("x-provider-header".to_string(), "safe".to_string()),
        ]);

        let headers = websocket_handshake_headers(&provider_headers, "invalid")
            .expect("provider headers should be valid");

        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer provider-token")
        );
        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("provider-api-key")
        );
        assert_eq!(
            headers.get("cookie").and_then(|value| value.to_str().ok()),
            Some("provider_session=provider-cookie")
        );
        assert_eq!(
            headers
                .get("x-provider-header")
                .and_then(|value| value.to_str().ok()),
            Some("safe")
        );
        for name in [
            "connection",
            "x-provider-hop",
            "upgrade",
            "sec-websocket-key",
            "sec-websocket-future-field",
            "proxy-authorization",
        ] {
            assert!(headers.get(name).is_none(), "{name} must not survive");
        }
    }
}
