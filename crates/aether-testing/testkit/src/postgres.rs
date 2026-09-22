use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static POSTGRES_WORKDIR_SEQ: AtomicU64 = AtomicU64::new(0);
const MAX_BIND_ATTEMPTS: usize = 3;
const MAX_DIAGNOSTIC_BYTES: u64 = 16 * 1024;

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
            Self::Io(error) => write!(formatter, "managed postgres launch I/O failed: {error}"),
            Self::Terminal {
                status, diagnostic, ..
            } => {
                write!(
                    formatter,
                    "managed postgres exited with {status}: {diagnostic}"
                )
            }
            Self::TimedOut { diagnostic } => {
                write!(
                    formatter,
                    "managed postgres readiness timed out: {diagnostic}"
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

use aether_data::driver::postgres::PostgresPoolConfig;
use aether_data::{DataBackends, DataLayerConfig};
use sqlx::{Connection, PgConnection};

use crate::server::reserve_local_port;

#[derive(Debug)]
pub struct ManagedPostgresServer {
    child: Option<Child>,
    postgres_bin: String,
    pg_ctl_bin: PathBuf,
    port: u16,
    workdir: PathBuf,
    data_dir: PathBuf,
    database_url: String,
    launch_attempt: u64,
    readiness_timeout: std::time::Duration,
}

impl ManagedPostgresServer {
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = reserve_local_port()?;
        // pid+port is not unique: cargo test shares one PID, and ephemeral ports
        // are reused after the listener is dropped. Parallel e2e tests then hit
        // create_dir AlreadyExists.
        let seq = POSTGRES_WORKDIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let workdir = std::env::temp_dir().join(format!(
            "aether-postgres-baseline-{}-{}-{}-{}",
            std::process::id(),
            port,
            seq,
            nanos
        ));
        let data_dir = workdir.join("data");
        std::fs::create_dir(&workdir)?;

        let initdb_bin = std::env::var("AETHER_INITDB_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "initdb".to_string());
        let postgres_bin = std::env::var("AETHER_POSTGRES_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "postgres".to_string());
        let pg_ctl_bin = std::env::var("AETHER_PG_CTL_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(&postgres_bin).with_file_name(if cfg!(windows) {
                    "pg_ctl.exe"
                } else {
                    "pg_ctl"
                })
            });
        let database_url = format!("postgres://aether@127.0.0.1:{port}/postgres");
        let mut server = Self {
            child: None,
            postgres_bin,
            pg_ctl_bin,
            port,
            workdir,
            data_dir,
            database_url,
            launch_attempt: 0,
            readiness_timeout: std::time::Duration::from_secs(10),
        };

        let init_output = Command::new(&initdb_bin)
            .arg("-D")
            .arg(&server.data_dir)
            .arg("-U")
            .arg("aether")
            .arg("--auth=trust")
            .arg("--encoding=UTF8")
            .arg("--no-locale")
            .arg("--no-instructions")
            .output()?;
        if !init_output.status.success() {
            return Err(std::io::Error::other(format!(
                "initdb failed: {}",
                String::from_utf8_lossy(&init_output.stderr)
            ))
            .into());
        }

        for attempt in 1..=MAX_BIND_ATTEMPTS {
            match server.launch_once().await {
                Ok(()) => return Ok(server),
                Err(error) if error.is_terminal_bind_collision() && attempt < MAX_BIND_ATTEMPTS => {
                    server.port = reserve_local_port()?;
                    server.database_url =
                        format!("postgres://aether@127.0.0.1:{}/postgres", server.port);
                }
                Err(error) => return Err(error.into()),
            }
        }
        unreachable!("the final launch attempt returns")
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(&mut self) -> Result<(), std::io::Error> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_some() {
            self.child = None;
            return Ok(());
        }

        let output = Command::new(&self.pg_ctl_bin)
            .arg("-D")
            .arg(&self.data_dir)
            .args(["stop", "-m", "fast", "-w", "-t", "10"])
            .output()?;
        if !output.status.success() && child.try_wait()?.is_none() {
            return Err(std::io::Error::other(format!(
                "pg_ctl stop failed for {}: {}{}",
                self.data_dir.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )));
        }
        child.wait()?;
        self.child = None;
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
            .join(format!("postgres.{}.log", self.launch_attempt));
        let stdout = std::fs::File::create(&log_path).map_err(LaunchFailure::Io)?;
        let stderr = stdout.try_clone().map_err(LaunchFailure::Io)?;
        let child = Command::new(&self.postgres_bin)
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-h")
            .arg("127.0.0.1")
            .arg("-p")
            .arg(self.port.to_string())
            .arg("-F")
            .arg("-c")
            .arg("unix_socket_directories=")
            .arg("-c")
            .arg("fsync=off")
            .arg("-c")
            .arg("synchronous_commit=off")
            .arg("-c")
            .arg("full_page_writes=off")
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
            if managed_postgres_matches_data_dir(&self.database_url, &self.data_dir, deadline).await
            {
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

async fn managed_postgres_matches_data_dir(
    database_url: &str,
    expected_data_dir: &Path,
    deadline: tokio::time::Instant,
) -> bool {
    tokio::time::timeout_at(deadline, async {
        let mut connection = PgConnection::connect(database_url).await.ok()?;
        let actual: String = sqlx::query_scalar("SHOW data_directory")
            .fetch_one(&mut connection)
            .await
            .ok()?;
        connection.close().await.ok()?;
        let actual = std::fs::canonicalize(actual).ok()?;
        let expected = std::fs::canonicalize(expected_data_dir).ok()?;
        Some(actual == expected)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

impl Drop for ManagedPostgresServer {
    fn drop(&mut self) {
        match self.stop() {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&self.workdir);
            }
            Err(error) => {
                eprintln!(
                    "failed to stop managed postgres; preserving {}: {error}",
                    self.workdir.display(),
                );
            }
        }
    }
}

pub async fn prepare_aether_postgres_schema(
    database_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = PostgresPoolConfig {
        database_url: database_url.to_string(),
        ..Default::default()
    };

    let backends = DataBackends::from_config(DataLayerConfig::from_postgres(config))?;
    let pending_migrations = backends
        .prepare_database_for_startup()
        .await?
        .unwrap_or_default();
    if !pending_migrations.is_empty() {
        backends.run_database_migrations().await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake_postgres(mode: &str) -> (ManagedPostgresServer, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);

        let root = std::env::temp_dir().join(format!(
            "aether-fake-postgres-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let data_dir = root.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let launcher = root.join("postgres");
        let count = root.join("count");
        let ports = root.join("ports");
        let pg_ctl = root.join("pg_ctl");
        let source = format!(
            r#"#!/usr/bin/env python3
import os, pathlib, socket, sys, time
count_path = pathlib.Path({count:?})
ports_path = pathlib.Path({ports:?})
try:
    count = int(count_path.read_text()) + 1
except Exception:
    count = 1
count_path.write_text(str(count))
pathlib.Path({pid:?}).write_text(str(os.getpid()))
port = sys.argv[sys.argv.index("-p") + 1]
with ports_path.open("a") as output:
    output.write(port + "\n")
mode = {mode:?}
if mode == "collision":
    print("could not bind IPv4 address: Address already in use", file=sys.stderr)
    sys.exit(1)
if mode == "nonbind":
    print("database files are incompatible", file=sys.stderr)
    sys.exit(2)
if mode == "blackhole":
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", int(port)))
    server.listen()
    client, _ = server.accept()
    pathlib.Path({accepted:?}).write_text("accepted")
    time.sleep(30)
time.sleep(30)
"#,
            count = count.to_string_lossy(),
            ports = ports.to_string_lossy(),
            pid = data_dir.join("postmaster.pid").to_string_lossy(),
            accepted = root.join("accepted").to_string_lossy(),
            mode = mode,
        );
        std::fs::write(&launcher, source).unwrap();
        let mut permissions = std::fs::metadata(&launcher).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&launcher, permissions).unwrap();
        std::fs::write(
            &pg_ctl,
            "#!/usr/bin/env python3\nimport os, pathlib, signal, sys\npid_file = pathlib.Path(sys.argv[sys.argv.index('-D') + 1]) / 'postmaster.pid'\nos.kill(int(pid_file.read_text()), signal.SIGTERM)\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&pg_ctl).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&pg_ctl, permissions).unwrap();
        let port = reserve_local_port().unwrap();
        (
            ManagedPostgresServer {
                child: None,
                postgres_bin: launcher.to_string_lossy().into_owned(),
                pg_ctl_bin: pg_ctl,
                port,
                workdir: root.clone(),
                data_dir,
                database_url: format!("postgres://aether@127.0.0.1:{port}/postgres"),
                launch_attempt: 0,
                readiness_timeout: std::time::Duration::from_secs(5),
            },
            count,
            ports,
        )
    }

    #[tokio::test(flavor = "current_thread")]
    #[cfg(unix)]
    async fn managed_postgres_restart_launch_loop_contract() {
        let (mut collision, count, ports) = fake_postgres("collision");
        let original_port = collision.port();
        let original_url = collision.database_url().to_string();
        let error = collision.restart().await.unwrap_err();
        assert!(error.to_string().contains("Address already in use"));
        assert_eq!(std::fs::read_to_string(count).unwrap(), "3");
        assert_eq!(
            std::fs::read_to_string(ports).unwrap(),
            format!("{original_port}\n{original_port}\n{original_port}\n")
        );
        assert_eq!(collision.port(), original_port);
        assert_eq!(collision.database_url(), original_url);

        let (mut nonbind, count, _) = fake_postgres("nonbind");
        let error = nonbind.restart().await.unwrap_err();
        assert!(error
            .to_string()
            .contains("database files are incompatible"));
        assert_eq!(std::fs::read_to_string(count).unwrap(), "1");

        let (mut timeout, count, _) = fake_postgres("timeout");
        let error = timeout.restart().await.unwrap_err();
        assert!(error.to_string().contains("readiness timed out"));
        assert_eq!(std::fs::read_to_string(count).unwrap(), "1");
        let mut owned_child = timeout.child.take().expect("timed-out child stays owned");
        owned_child.kill().unwrap();
        owned_child.wait().unwrap();

        let (mut blackhole, count, _) = fake_postgres("blackhole");
        let started = std::time::Instant::now();
        let error = blackhole.restart().await.unwrap_err();
        assert!(error.to_string().contains("readiness timed out"));
        assert_eq!(std::fs::read_to_string(count).unwrap(), "1");
        assert!(started.elapsed() < std::time::Duration::from_secs(7));
        assert!(
            blackhole.workdir.join("accepted").exists(),
            "probe reached the blackhole"
        );
        let mut owned_child = blackhole.child.take().expect("blackhole child stays owned");
        owned_child.kill().unwrap();
        owned_child.wait().unwrap();
    }

    #[test]
    fn bind_retry_classifier_requires_explicit_diagnostic() {
        assert!(diagnostic_is_bind_collision(
            "could not bind IPv4 address 127.0.0.1: Address already in use"
        ));
        assert!(!diagnostic_is_bind_collision(
            "database files are incompatible"
        ));
        assert!(!diagnostic_is_bind_collision("authentication failed"));
    }

    #[tokio::test]
    #[ignore = "requires local initdb, postgres, and pg_ctl binaries"]
    async fn live_managed_postgres_restarts_cleanly_with_open_connections() {
        let mut server = ManagedPostgresServer::start().await.unwrap();
        let workdir = server.workdir.clone();
        let mut connection = PgConnection::connect(server.database_url()).await.unwrap();
        sqlx::query("CREATE TABLE restart_probe (value INTEGER NOT NULL)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO restart_probe VALUES (42)")
            .execute(&mut connection)
            .await
            .unwrap();

        for _iteration in 0..4 {
            server.stop().unwrap();
            server.stop().unwrap();
            assert!(server.child.is_none());
            assert!(!server.data_dir.join("postmaster.pid").exists());
            assert!(server.data_dir.exists());
            server.restart().await.unwrap();
            connection = PgConnection::connect(server.database_url()).await.unwrap();
            let value: i32 = sqlx::query_scalar("SELECT value FROM restart_probe")
                .fetch_one(&mut connection)
                .await
                .unwrap();
            assert_eq!(value, 42);
        }

        drop(server);
        assert!(!workdir.exists());
    }

    #[tokio::test]
    #[ignore = "requires local initdb, postgres, and pg_ctl binaries"]
    async fn live_failed_postgres_stop_can_be_retried_without_losing_ownership() {
        let mut server = ManagedPostgresServer::start().await.unwrap();
        let pg_ctl_bin = server.pg_ctl_bin.clone();
        server.pg_ctl_bin = server.workdir.join("missing-pg-ctl");
        assert!(server.stop().is_err());
        assert!(server.child.as_mut().unwrap().try_wait().unwrap().is_none());
        assert!(server.data_dir.exists());
        server.pg_ctl_bin = pg_ctl_bin;
        server.stop().unwrap();
        assert!(server.child.is_none());
        assert!(!server.data_dir.join("postmaster.pid").exists());
    }
}
