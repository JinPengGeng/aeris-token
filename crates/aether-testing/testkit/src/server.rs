use std::fmt;
use std::net::SocketAddr;

use axum::Router;

pub struct ReservedListener {
    listener: tokio::net::TcpListener,
    addr: SocketAddr,
}

impl ReservedListener {
    pub async fn bind() -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        Ok(Self { listener, addr })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

pub struct SpawnedServer {
    base_url: String,
    port: u16,
    handle: tokio::task::JoinHandle<()>,
}

impl SpawnedServer {
    pub async fn start(app: Router) -> Result<Self, std::io::Error> {
        Self::start_on_port(0, app).await
    }

    pub async fn start_on_port(port: u16, app: Router) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        Self::start_bound(listener, addr, app)
    }

    pub fn start_with_listener(
        reservation: ReservedListener,
        app: Router,
    ) -> Result<Self, std::io::Error> {
        Self::start_bound(reservation.listener, reservation.addr, app)
    }

    fn start_bound(
        listener: tokio::net::TcpListener,
        addr: SocketAddr,
        app: Router,
    ) -> Result<Self, std::io::Error> {
        let handle = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("spawned server should run");
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            port: addr.port(),
            handle,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Debug for SpawnedServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnedServer")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Drop for SpawnedServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub fn reserve_local_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn reserved_listener_stays_owned_until_server_handoff() {
        let reservation = ReservedListener::bind().await.unwrap();
        let port = reservation.port();
        let competing = tokio::net::TcpListener::bind(("127.0.0.1", port)).await;
        assert_eq!(competing.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);

        let server = SpawnedServer::start_with_listener(
            reservation,
            Router::new().route("/probe", get(|| async { "ok" })),
        )
        .unwrap();
        assert_eq!(server.port(), port);

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"));
    }

    #[tokio::test]
    async fn dropping_unconsumed_reservation_releases_port() {
        let reservation = ReservedListener::bind().await.unwrap();
        let port = reservation.port();
        drop(reservation);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }
}
