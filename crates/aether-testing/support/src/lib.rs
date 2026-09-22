use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Holds a loopback listener bound to an ephemeral port so the port stays
/// reserved until [`PortReservation::release`] is called.
///
/// External server processes (redis, postgres) cannot take over a pre-bound
/// listener, so callers hold the reservation across slow setup work (workdir
/// creation, initdb) and release it immediately before spawning the child.
/// That shrinks the classic "probe a free port, drop, rebind" TOCTOU window
/// to the spawn call itself; collisions after release are still retried by
/// the managed-server launch loops.
#[derive(Debug)]
pub struct PortReservation {
    listener: std::net::TcpListener,
    port: u16,
}

impl PortReservation {
    pub fn bind() -> Result<Self, std::io::Error> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Consumes the reservation, returning the port. Call this as the last
    /// step before the process that will re-bind the port is spawned.
    pub fn release(self) -> u16 {
        self.port
    }
}

/// Single in-workspace implementation of a scratch redis-server lifecycle
/// for tests. `aether-testkit` and `aether-loadtools` both re-export this
/// type; do not add a second copy (issue #212).
#[derive(Debug)]
pub struct ManagedRedisServer {
    child: Option<Child>,
    binary: String,
    port: u16,
    workdir: PathBuf,
    redis_url: String,
    launch_attempt: u64,
    readiness_timeout: std::time::Duration,
}

const MAX_BIND_ATTEMPTS: usize = 3;
const MAX_DIAGNOSTIC_BYTES: u64 = 16 * 1024;
static REDIS_WORKDIR_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
enum LaunchFailure {
    Io(std::io::Error),
    Terminal {
        status: ExitStatus,
        diagnostic: String,
        bind_collision: bool,
    },
    TimedOut {
        diagnostic: String,
    },
}

impl LaunchFailure {
    fn is_terminal_bind_collision(&self) -> bool {
        matches!(
            self,
            Self::Terminal {
                bind_collision: true,
                ..
            }
        )
    }
}

impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "managed service launch I/O failed: {error}"),
            Self::Terminal {
                status, diagnostic, ..
            } => {
                write!(
                    formatter,
                    "managed service exited with {status}: {diagnostic}"
                )
            }
            Self::TimedOut { diagnostic } => {
                write!(
                    formatter,
                    "managed service readiness timed out: {diagnostic}"
                )
            }
        }
    }
}

impl std::error::Error for LaunchFailure {}

fn diagnostic_is_bind_collision(diagnostic: &str) -> bool {
    let normalized = diagnostic.to_ascii_lowercase();
    normalized.contains("address already in use") || normalized.contains("eaddrinuse")
}

fn read_diagnostic_tail(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(MAX_DIAGNOSTIC_BYTES)))?;
    let mut bytes = Vec::with_capacity(len.min(MAX_DIAGNOSTIC_BYTES) as usize);
    file.take(MAX_DIAGNOSTIC_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

impl ManagedRedisServer {
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let binary = std::env::var("AETHER_REDIS_SERVER_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "redis-server".to_string());
        Self::start_with_binary(binary, std::time::Duration::from_secs(5)).await
    }

    async fn start_with_binary(
        binary: String,
        readiness_timeout: std::time::Duration,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Hold the reservation across workdir setup so a concurrent test
        // cannot claim the port while we prepare the launch.
        let reservation = PortReservation::bind()?;
        let workdir = Self::create_workdir()?;
        let port = reservation.release();
        Self::launch_in_workdir(binary, readiness_timeout, port, workdir).await
    }

    async fn start_with_binary_on_port(
        binary: String,
        readiness_timeout: std::time::Duration,
        port: u16,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let workdir = Self::create_workdir()?;
        Self::launch_in_workdir(binary, readiness_timeout, port, workdir).await
    }

    fn create_workdir() -> Result<PathBuf, std::io::Error> {
        // A bind retry can change ports, and concurrent starts may reserve the
        // same initial port. Directory ownership must not depend on that port.
        let seq = REDIS_WORKDIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let workdir = std::env::temp_dir().join(format!(
            "aether-redis-test-{}-{seq}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir(&workdir)?;
        Ok(workdir)
    }

    async fn launch_in_workdir(
        binary: String,
        readiness_timeout: std::time::Duration,
        port: u16,
        workdir: PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let redis_url = format!("redis://127.0.0.1:{port}/0");
        let mut server = Self {
            child: None,
            binary,
            port,
            workdir,
            redis_url,
            launch_attempt: 0,
            readiness_timeout,
        };
        for attempt in 1..=MAX_BIND_ATTEMPTS {
            match server.launch_once().await {
                Ok(()) => return Ok(server),
                Err(error) if error.is_terminal_bind_collision() && attempt < MAX_BIND_ATTEMPTS => {
                    // Reserve and release back-to-back right before the next
                    // launch attempt: the only exposed window is the spawn.
                    server.port = PortReservation::bind()?.release();
                    server.redis_url = format!("redis://127.0.0.1:{}/0", server.port);
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("the final launch attempt returns")
    }

    pub fn redis_url(&self) -> &str {
        &self.redis_url
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(&mut self) -> Result<(), std::io::Error> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    pub async fn restart(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.stop()?;
        for attempt in 1..=MAX_BIND_ATTEMPTS {
            match self.launch_once().await {
                Ok(()) => return Ok(()),
                Err(error) if error.is_terminal_bind_collision() && attempt < MAX_BIND_ATTEMPTS => {
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("the final launch attempt returns")
    }

    async fn launch_once(&mut self) -> Result<(), LaunchFailure> {
        self.launch_attempt += 1;
        let log_path = self
            .workdir
            .join(format!("redis.stderr.{}.log", self.launch_attempt));
        let stdout = std::fs::File::create(&log_path).map_err(LaunchFailure::Io)?;
        let stderr = stdout.try_clone().map_err(LaunchFailure::Io)?;
        let child = Command::new(&self.binary)
            .arg("--save")
            .arg("")
            .arg("--appendonly")
            .arg("no")
            .arg("--port")
            .arg(self.port.to_string())
            .arg("--dir")
            .arg(&self.workdir)
            .arg("--bind")
            .arg("127.0.0.1")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(LaunchFailure::Io)?;
        self.child = Some(child);

        let deadline = tokio::time::Instant::now() + self.readiness_timeout;
        while tokio::time::Instant::now() < deadline {
            if let Some(failure) = self.terminal_failure(&log_path)? {
                return Err(failure);
            }
            let owned_pid = self
                .child
                .as_ref()
                .expect("launch child should be owned")
                .id();
            if redis_process_id(("127.0.0.1", self.port), deadline).await == Some(owned_pid) {
                return match self.terminal_failure(&log_path)? {
                    Some(failure) => Err(failure),
                    None => Ok(()),
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Err(LaunchFailure::TimedOut {
            diagnostic: read_diagnostic_tail(&log_path).unwrap_or_default(),
        })
    }

    fn terminal_failure(
        &mut self,
        log_path: &Path,
    ) -> Result<Option<LaunchFailure>, LaunchFailure> {
        let Some(status) = self
            .child
            .as_mut()
            .expect("launch child should be owned")
            .try_wait()
            .map_err(LaunchFailure::Io)?
        else {
            return Ok(None);
        };
        self.child = None;
        let diagnostic = read_diagnostic_tail(log_path).unwrap_or_default();
        Ok(Some(LaunchFailure::Terminal {
            status,
            bind_collision: diagnostic_is_bind_collision(&diagnostic),
            diagnostic,
        }))
    }
}

impl Drop for ManagedRedisServer {
    fn drop(&mut self) {
        let _ = self.stop();
        let _ = std::fs::remove_dir_all(&self.workdir);
    }
}

async fn redis_process_id(addr: (&str, u16), deadline: tokio::time::Instant) -> Option<u32> {
    tokio::time::timeout_at(deadline, async move {
        let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
        stream
            .write_all(b"*2\r\n$4\r\nINFO\r\n$6\r\nserver\r\n")
            .await
            .ok()?;
        let mut response = Vec::with_capacity(4096);
        loop {
            let mut chunk = [0_u8; 1024];
            let len = stream.read(&mut chunk).await.ok()?;
            if len == 0 {
                break;
            }
            response.extend_from_slice(&chunk[..len]);
            if response.len() >= 16 * 1024 {
                return None;
            }
            match parse_redis_info_process_id(&response) {
                Ok(Some(process_id)) => return Some(process_id),
                Ok(None) => {}
                Err(()) => return None,
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

fn parse_redis_info_process_id(response: &[u8]) -> Result<Option<u32>, ()> {
    if response.first() != Some(&b'$') {
        return Err(());
    }
    let header_end = response
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|index| index + 2);
    let Some(header_end) = header_end else {
        return Ok(None);
    };
    let payload_len: usize = std::str::from_utf8(&response[1..header_end - 2])
        .map_err(|_| ())?
        .parse()
        .map_err(|_| ())?;
    if payload_len > 16 * 1024 {
        return Err(());
    }
    let frame_len = header_end
        .checked_add(payload_len)
        .and_then(|len| len.checked_add(2))
        .ok_or(())?;
    if response.len() < frame_len {
        return Ok(None);
    }
    if &response[header_end + payload_len..frame_len] != b"\r\n" || response.len() != frame_len {
        return Err(());
    }
    let payload =
        std::str::from_utf8(&response[header_end..header_end + payload_len]).map_err(|_| ())?;
    let value = payload
        .split("\r\n")
        .find_map(|line| line.strip_prefix("process_id:"))
        .ok_or(())?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    value.parse().map(Some).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_reservation_blocks_rebind_until_released() {
        let reservation = PortReservation::bind().unwrap();
        let port = reservation.port();
        let competing = std::net::TcpListener::bind(("127.0.0.1", port));
        assert_eq!(competing.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);

        let port_after_release = reservation.release();
        assert_eq!(port_after_release, port);
        let rebound = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(rebound.is_ok());
    }

    #[test]
    fn concurrent_port_reservations_are_distinct() {
        let reservations: Vec<_> = (0..32).map(|_| PortReservation::bind().unwrap()).collect();
        let mut ports: Vec<u16> = reservations
            .iter()
            .map(|reservation| reservation.port())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), 32, "reservations must not share ports");
    }

    #[cfg(unix)]
    fn fake_launcher(mode: &str) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);

        let root = std::env::temp_dir().join(format!(
            "aether-fake-redis-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let launcher = root.join("redis-server");
        let count = root.join("count");
        let source = format!(
            r#"#!/usr/bin/env python3
import os, pathlib, socket, sys, time
count_path = pathlib.Path({count:?})
try:
    count = int(count_path.read_text()) + 1
except Exception:
    count = 1
count_path.write_text(str(count))
pathlib.Path({pid:?}).write_text(str(os.getpid()))
mode = {mode:?}
if mode == "always_collision" or (mode == "collision_then_serve" and count == 1):
    print("Could not create server TCP listening socket: Address already in use")
    sys.exit(1)
if mode == "nonbind":
    print("configuration rejected", file=sys.stderr)
    sys.exit(2)
if mode == "timeout":
    time.sleep(30)
    sys.exit(0)
port = int(sys.argv[sys.argv.index("--port") + 1])
server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(("127.0.0.1", port))
server.listen()
while True:
    client, _ = server.accept()
    client.recv(1024)
    if mode == "blackhole":
        pathlib.Path({accepted:?}).write_text("accepted")
        time.sleep(30)
        continue
    payload = ('# Server\r\nprocess_id:' + str(os.getpid()) + '\r\n').encode()
    client.sendall(("$" + str(len(payload)) + "\r\n").encode() + payload + b"\r\n")
    client.close()
"#,
            count = count.to_string_lossy(),
            pid = root.join("pid").to_string_lossy(),
            accepted = root.join("accepted").to_string_lossy(),
            mode = mode,
        );
        std::fs::write(&launcher, source).unwrap();
        let mut permissions = std::fs::metadata(&launcher).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&launcher, permissions).unwrap();
        (launcher, count, root)
    }

    #[cfg(unix)]
    fn launch_count(path: &Path) -> usize {
        std::fs::read_to_string(path).unwrap().parse().unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg(unix)]
    async fn managed_redis_launch_loop_contract() {
        // Include interpreter startup under instrumented Cargo environments.
        let short = std::time::Duration::from_secs(5);

        let (launcher, count, root) = fake_launcher("collision_then_serve");
        let mut server =
            ManagedRedisServer::start_with_binary(launcher.to_string_lossy().into_owned(), short)
                .await
                .unwrap();
        assert_eq!(launch_count(&count), 2);
        let port = server.port();
        let url = server.redis_url().to_string();
        server.restart().await.unwrap();
        assert_eq!(server.port(), port);
        assert_eq!(server.redis_url(), url);
        assert_eq!(launch_count(&count), 3);
        drop(server);
        std::fs::remove_dir_all(root).unwrap();

        let (launcher, count, root) = fake_launcher("always_collision");
        let error =
            ManagedRedisServer::start_with_binary(launcher.to_string_lossy().into_owned(), short)
                .await
                .unwrap_err();
        assert!(error.to_string().contains("Address already in use"));
        assert_eq!(launch_count(&count), MAX_BIND_ATTEMPTS);
        std::fs::remove_dir_all(root).unwrap();

        let (launcher, count, root) = fake_launcher("nonbind");
        let error =
            ManagedRedisServer::start_with_binary(launcher.to_string_lossy().into_owned(), short)
                .await
                .unwrap_err();
        assert!(error.to_string().contains("configuration rejected"));
        assert_eq!(launch_count(&count), 1);
        std::fs::remove_dir_all(root).unwrap();

        let (launcher, count, root) = fake_launcher("timeout");
        let started = std::time::Instant::now();
        let error =
            ManagedRedisServer::start_with_binary(launcher.to_string_lossy().into_owned(), short)
                .await
                .unwrap_err();
        assert!(error.to_string().contains("readiness timed out"));
        assert_eq!(launch_count(&count), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(7));
        std::fs::remove_dir_all(root).unwrap();

        let (launcher, count, root) = fake_launcher("blackhole");
        let started = std::time::Instant::now();
        let error =
            ManagedRedisServer::start_with_binary(launcher.to_string_lossy().into_owned(), short)
                .await
                .unwrap_err();
        assert!(error.to_string().contains("readiness timed out"));
        assert_eq!(launch_count(&count), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(7));
        assert!(
            root.join("accepted").exists(),
            "probe reached the blackhole"
        );
        std::fs::remove_dir_all(root).unwrap();

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let external_port = listener.local_addr().unwrap().port();
        let (launcher, count, root) = fake_launcher("timeout");
        let owned_pid_path = root.join("pid");
        let external = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let owned_pid_path = owned_pid_path.clone();
                tokio::spawn(async move {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request).await;
                    let owned_pid = loop {
                        if let Ok(value) = std::fs::read_to_string(&owned_pid_path) {
                            break value;
                        }
                        tokio::task::yield_now().await;
                    };
                    let payload = format!("# Server\r\nprocess_id:{}9\r\n", owned_pid.trim());
                    let frame = format!("${}\r\n{}\r\n", payload.len(), payload);
                    let split = frame.find(owned_pid.trim()).unwrap() + owned_pid.trim().len();
                    let _ = stream.write_all(&frame.as_bytes()[..split]).await;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    let _ = stream.write_all(&frame.as_bytes()[split..]).await;
                });
            }
        });
        let error = ManagedRedisServer::start_with_binary_on_port(
            launcher.to_string_lossy().into_owned(),
            short,
            external_port,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("readiness timed out"));
        assert_eq!(launch_count(&count), 1);
        external.abort();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg(unix)]
    async fn collided_initial_ports_keep_independent_workdir_ownership() {
        let timeout = std::time::Duration::from_secs(5);
        let (launcher_a, _, root_a) = fake_launcher("serve");
        let mut first = ManagedRedisServer::start_with_binary(
            launcher_a.to_string_lossy().into_owned(),
            timeout,
        )
        .await
        .unwrap();
        let first_port = first.port();
        let first_dir = first.workdir.clone();
        let first_log = std::fs::read(first_dir.join("redis.stderr.1.log")).unwrap();
        std::fs::write(first_dir.join("owned-marker"), "first").unwrap();

        let (launcher_b, count_b, root_b) = fake_launcher("serve");
        let second = ManagedRedisServer::start_with_binary_on_port(
            launcher_b.to_string_lossy().into_owned(),
            timeout,
            first_port,
        )
        .await
        .unwrap();
        assert_eq!(launch_count(&count_b), 2);
        assert_ne!(first.port(), second.port());
        assert_ne!(first.workdir, second.workdir);
        let second_dir = second.workdir.clone();
        drop(second);
        assert!(!second_dir.exists());
        assert_eq!(
            std::fs::read_to_string(first_dir.join("owned-marker")).unwrap(),
            "first"
        );
        assert_eq!(
            std::fs::read(first_dir.join("redis.stderr.1.log")).unwrap(),
            first_log
        );
        first.restart().await.unwrap();
        assert_eq!(first.port(), first_port);
        drop(first);
        assert!(!first_dir.exists());
        std::fs::remove_dir_all(root_a).unwrap();
        std::fs::remove_dir_all(root_b).unwrap();
    }

    #[test]
    fn redis_info_parser_waits_for_complete_pid_line_and_bulk_frame() {
        let payload = b"# Server\r\nprocess_id:12345\r\n";
        let mut frame = format!("${}\r\n", payload.len()).into_bytes();
        frame.extend_from_slice(payload);
        frame.extend_from_slice(b"\r\n");
        let colon_split = frame
            .windows(b"process_id:".len())
            .position(|window| window == b"process_id:")
            .unwrap()
            + b"process_id:".len();
        let digit_split = colon_split + 3;
        assert_eq!(parse_redis_info_process_id(&frame[..colon_split]), Ok(None));
        assert_eq!(parse_redis_info_process_id(&frame[..digit_split]), Ok(None));
        assert_eq!(parse_redis_info_process_id(&frame), Ok(Some(12345)));
    }

    #[test]
    fn bind_retry_classifier_requires_explicit_diagnostic() {
        assert!(diagnostic_is_bind_collision(
            "Could not create server TCP listening socket: Address already in use"
        ));
        assert!(diagnostic_is_bind_collision("listen EADDRINUSE"));
        assert!(!diagnostic_is_bind_collision("permission denied"));
        assert!(!diagnostic_is_bind_collision("authentication failed"));
    }

    #[test]
    fn diagnostic_tail_is_bounded_and_keeps_latest_failure() {
        let path = std::env::temp_dir().join(format!(
            "aether-bind-diagnostic-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let mut content = vec![b'x'; (MAX_DIAGNOSTIC_BYTES * 2) as usize];
        content.extend_from_slice(b"Address already in use");
        std::fs::write(&path, content).unwrap();
        let tail = read_diagnostic_tail(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(tail.len() <= MAX_DIAGNOSTIC_BYTES as usize);
        assert!(tail.ends_with("Address already in use"));
    }
}
