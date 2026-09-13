//! Service installation and management for `aether-tunnel`.
//!
//! Supports the host-native service manager we currently target:
//! `systemd` on most Linux distributions and `OpenRC` on Alpine.

#[cfg(unix)]
use std::fs::OpenOptions;
use std::io::ErrorKind;
#[cfg(unix)]
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

const SERVICE_NAME: &str = "aether-tunnel";
const SERVICE_USER: &str = "aether-tunnel";
const SERVICE_GROUP: &str = "aether-tunnel";

const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/aether-tunnel.service";

const OPENRC_INIT_PATH: &str = "/etc/init.d/aether-tunnel";
const OPENRC_PID_PATH: &str = "/run/aether-tunnel.pid";
const OPENRC_LOG_DIR: &str = "/var/log/aether-tunnel";
const OPENRC_STDOUT_LOG: &str = "/var/log/aether-tunnel/current.log";
const OPENRC_STDERR_LOG: &str = "/var/log/aether-tunnel/error.log";

const SYSTEMCTL_BINS: &[&str] = &[
    "/usr/bin/systemctl",
    "/bin/systemctl",
    "/run/current-system/sw/bin/systemctl",
];
const JOURNALCTL_BINS: &[&str] = &[
    "/usr/bin/journalctl",
    "/bin/journalctl",
    "/run/current-system/sw/bin/journalctl",
];
const OPENRC_RUN_BINS: &[&str] = &["/sbin/openrc-run", "/usr/sbin/openrc-run", "openrc-run"];
const OPENRC_SERVICE_BINS: &[&str] = &["/sbin/rc-service", "/usr/sbin/rc-service", "rc-service"];
const OPENRC_UPDATE_BINS: &[&str] = &["/sbin/rc-update", "/usr/sbin/rc-update", "rc-update"];
const OPENRC_SUPERVISE_BINS: &[&str] = &[
    "/sbin/supervise-daemon",
    "/usr/sbin/supervise-daemon",
    "supervise-daemon",
];
const TAIL_BINS: &[&str] = &["/usr/bin/tail", "/bin/tail", "tail"];
const ID_BINS: &[&str] = &["/usr/bin/id", "/bin/id"];
const GROUPADD_BINS: &[&str] = &["/usr/sbin/groupadd", "/usr/bin/groupadd"];
const USERADD_BINS: &[&str] = &["/usr/sbin/useradd", "/usr/bin/useradd"];
const ADDGROUP_BINS: &[&str] = &["/sbin/addgroup", "/usr/sbin/addgroup"];
const ADDUSER_BINS: &[&str] = &["/sbin/adduser", "/usr/sbin/adduser"];
const CHOWN_BINS: &[&str] = &["/usr/bin/chown", "/bin/chown"];
const GETENT_BINS: &[&str] = &["/usr/bin/getent", "/bin/getent"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceManager {
    Systemd,
    OpenRc,
}

impl ServiceManager {
    fn display_name(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::OpenRc => "OpenRC",
        }
    }

    fn unit_path(self) -> &'static str {
        match self {
            Self::Systemd => SYSTEMD_UNIT_PATH,
            Self::OpenRc => OPENRC_INIT_PATH,
        }
    }

    fn is_installed(self) -> bool {
        Path::new(self.unit_path()).exists()
    }
}

pub fn is_available() -> bool {
    detect_service_manager().is_some() && is_root()
}

pub fn preferred_manager_name() -> &'static str {
    installed_manager()
        .or_else(detect_service_manager)
        .map(ServiceManager::display_name)
        .unwrap_or("service")
}

pub fn unavailable_hint() -> String {
    match detect_service_manager() {
        Some(manager) if !is_root() => {
            format!(
                "requires root with {}, use: sudo aether-tunnel setup",
                manager.display_name()
            )
        }
        Some(manager) => format!(
            "{} is available but service setup is not ready",
            manager.display_name()
        ),
        None => "no supported service manager detected (systemd/OpenRC)".into(),
    }
}

pub fn install_service(config_path: &Path) -> anyhow::Result<()> {
    let manager = detect_service_manager()
        .ok_or_else(|| anyhow::anyhow!("no supported service manager detected (systemd/OpenRC)"))?;

    if !is_root() {
        anyhow::bail!("root required, use: sudo ./aether-tunnel setup");
    }

    match manager {
        ServiceManager::Systemd => install_systemd_service(config_path),
        ServiceManager::OpenRc => install_openrc_service(config_path),
    }
}

pub(crate) fn is_root() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

pub fn is_installed() -> bool {
    installed_manager().is_some()
}

pub fn is_service_active() -> bool {
    active_service_manager().is_some()
}

pub fn restart_active_service() -> anyhow::Result<()> {
    let manager =
        active_service_manager().ok_or_else(|| anyhow::anyhow!("no active service detected"))?;
    restart_manager(manager)
}

pub fn uninstall_service() -> anyhow::Result<()> {
    let Some(manager) = installed_manager() else {
        return Ok(());
    };

    match manager {
        ServiceManager::Systemd => uninstall_systemd_service(),
        ServiceManager::OpenRc => uninstall_openrc_service(),
    }
}

pub fn cmd_status() -> anyhow::Result<()> {
    let manager = ensure_service_installed()?;
    let status = manager_status(manager)?;
    std::process::exit(status.code().unwrap_or(1));
}

pub fn cmd_logs() -> anyhow::Result<()> {
    let manager = ensure_service_installed()?;
    if manager == ServiceManager::OpenRc {
        ensure_openrc_logs_readable()?;
    }
    let status = match manager {
        ServiceManager::Systemd => Command::new(journalctl_bin())
            .args(["-u", SERVICE_NAME, "-f", "--no-pager", "-n", "100"])
            .status()?,
        ServiceManager::OpenRc => Command::new(tail_bin())
            .args(["-n", "100", "-f", OPENRC_STDOUT_LOG, OPENRC_STDERR_LOG])
            .status()?,
    };
    std::process::exit(status.code().unwrap_or(1));
}

pub fn cmd_start() -> anyhow::Result<()> {
    let manager = ensure_root_and_service()?;
    start_manager(manager)?;
    eprintln!("  Service started.");
    Ok(())
}

pub fn cmd_restart() -> anyhow::Result<()> {
    let manager = ensure_root_and_service()?;
    restart_manager(manager)?;
    eprintln!("  Service restarted.");
    Ok(())
}

pub fn cmd_stop() -> anyhow::Result<()> {
    let manager = ensure_root_and_service()?;
    stop_manager(manager)?;
    eprintln!("  Service stopped.");
    Ok(())
}

pub fn cmd_uninstall() -> anyhow::Result<()> {
    ensure_root_and_service()?;
    uninstall_service()?;
    eprintln!();
    eprintln!("  Config file, TLS certs, and logs are preserved. Remove manually if needed.");
    Ok(())
}

pub(crate) fn run_cmd(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let display = format!("{} {}", program, args.join(" "));
    eprintln!("  > {}", display);

    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        anyhow::bail!("command failed: {}", display);
    }
    Ok(())
}

fn detect_service_manager() -> Option<ServiceManager> {
    if is_systemd_available() {
        Some(ServiceManager::Systemd)
    } else if is_openrc_available() {
        Some(ServiceManager::OpenRc)
    } else {
        None
    }
}

fn installed_manager() -> Option<ServiceManager> {
    if let Some(manager) = detect_service_manager() {
        if manager.is_installed() {
            return Some(manager);
        }
    }

    [ServiceManager::Systemd, ServiceManager::OpenRc]
        .into_iter()
        .find(|manager| manager.is_installed())
}

fn ensure_openrc_logs_readable() -> anyhow::Result<()> {
    for path in [OPENRC_STDOUT_LOG, OPENRC_STDERR_LOG] {
        match std::fs::File::open(path) {
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                anyhow::bail!(
                    "OpenRC logs are stored under {} and usually require root access. Try `sudo ./aether-tunnel logs`.",
                    OPENRC_LOG_DIR
                );
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                anyhow::bail!(
                    "OpenRC log file not found at {}. Start the service first or check `./aether-tunnel status`.",
                    path
                );
            }
            Err(err) => return Err(err.into()),
        }
    }

    Ok(())
}

fn active_service_manager() -> Option<ServiceManager> {
    if let Some(manager) = installed_manager() {
        if manager_is_active(manager) {
            return Some(manager);
        }
    }

    [ServiceManager::Systemd, ServiceManager::OpenRc]
        .into_iter()
        .find(|manager| manager_is_active(*manager))
}

fn ensure_service_installed() -> anyhow::Result<ServiceManager> {
    installed_manager().ok_or_else(|| {
        anyhow::anyhow!("service not installed, run `sudo ./aether-tunnel setup` first")
    })
}

fn ensure_root_and_service() -> anyhow::Result<ServiceManager> {
    let manager = ensure_service_installed()?;
    if !is_root() {
        anyhow::bail!("root required, use: sudo ./aether-tunnel <command>");
    }
    Ok(manager)
}

fn install_systemd_service(config_path: &Path) -> anyhow::Result<()> {
    let exe_path = std::env::current_exe()?.canonicalize()?;
    let exe_str = exe_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("binary path contains invalid UTF-8"))?;

    let config_abs = prepare_service_config_path(config_path)?;
    let config_str = config_abs
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("config path contains invalid UTF-8"))?;

    let working_dir = config_abs
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_str()
        .unwrap_or("/");

    validate_service_unit_path(exe_str, "binary")?;
    validate_service_unit_path(config_str, "config")?;
    validate_service_unit_path(working_dir, "working directory")?;
    ensure_service_identity()?;
    migrate_service_permissions(&config_abs)?;
    ensure_private_service_directory(Path::new(OPENRC_LOG_DIR), 0o750)?;
    migrate_service_log_permissions()?;
    validate_root_managed_service_file(&exe_path, "binary", false)?;
    validate_service_config_file(&config_abs)?;

    if Path::new(SYSTEMD_UNIT_PATH).exists() {
        eprintln!("  Stopping existing service...");
        let _ = Command::new(systemctl_bin())
            .args(["stop", SERVICE_NAME])
            .status();
    }

    eprintln!("  Generating systemd unit file...");
    eprintln!("    Binary:  {}", exe_str);
    eprintln!("    Config:  {}", config_str);
    eprintln!("    WorkDir: {}", working_dir);

    let unit_content = render_systemd_unit(exe_str, config_str, working_dir)?;
    write_service_definition(SYSTEMD_UNIT_PATH, &unit_content, 0o644)?;

    eprintln!("  Enabling and starting service...");
    run_cmd(systemctl_bin(), &["daemon-reload"])?;
    run_cmd(systemctl_bin(), &["enable", "--now", SERVICE_NAME])?;

    eprintln!();
    if manager_is_active(ServiceManager::Systemd) {
        eprintln!("  Service started successfully!");
    } else {
        eprintln!("  Service state is not active yet. Check `sudo ./aether-tunnel logs`.");
    }

    print_post_install_commands();
    Ok(())
}

fn render_systemd_unit(
    exe_path: &str,
    config_path: &str,
    working_dir: &str,
) -> anyhow::Result<String> {
    validate_service_unit_path(exe_path, "binary")?;
    validate_service_unit_path(config_path, "config")?;
    validate_service_unit_path(working_dir, "working directory")?;

    let exe_path = systemd_quote(exe_path);
    let working_dir = systemd_quote(working_dir);
    let config_env = systemd_quote(&format!("AETHER_TUNNEL_CONFIG={config_path}"));
    Ok(format!(
        "[Unit]\n\
         Description=Aether Tunnel\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={working_dir}\n\
         Environment={config_env}\n\
         Environment=AETHER_TUNNEL_SERVICE_MANAGER=systemd\n\
         Environment=AETHER_TUNNEL_LOG_DESTINATION=both\n\
         Environment=AETHER_TUNNEL_LOG_DIR=/var/log/aether-tunnel\n\
         ExecStart={exe_path}\n\
         User={SERVICE_USER}\n\
         Group={SERVICE_GROUP}\n\
         NoNewPrivileges=true\n\
         PrivateTmp=true\n\
         ProtectSystem=strict\n\
         ProtectHome=true\n\
         ReadWritePaths=/var/log/aether-tunnel\n\
         CapabilityBoundingSet=\n\
         AmbientCapabilities=\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         LimitNOFILE=65535\n\
         UMask=0077\n\
         LogsDirectory=aether-tunnel\n\
         LogsDirectoryMode=0750\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
    ))
}

fn install_openrc_service(config_path: &Path) -> anyhow::Result<()> {
    let exe_path = std::env::current_exe()?.canonicalize()?;
    let exe_str = exe_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("binary path contains invalid UTF-8"))?;

    let config_abs = prepare_service_config_path(config_path)?;
    let config_str = config_abs
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("config path contains invalid UTF-8"))?;

    let working_dir = config_abs
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_str()
        .unwrap_or("/");

    validate_service_unit_path(exe_str, "binary")?;
    validate_service_unit_path(config_str, "config")?;
    validate_service_unit_path(working_dir, "working directory")?;
    ensure_service_identity()?;
    migrate_service_permissions(&config_abs)?;
    validate_root_managed_service_file(&exe_path, "binary", false)?;
    validate_service_config_file(&config_abs)?;

    if Path::new(OPENRC_INIT_PATH).exists() {
        eprintln!("  Stopping existing service...");
        let _ = Command::new(openrc_service_bin())
            .args([SERVICE_NAME, "stop"])
            .status();
    }

    ensure_private_service_directory(Path::new(OPENRC_LOG_DIR), 0o750)?;
    open_private_service_log(Path::new(OPENRC_STDOUT_LOG), 0o640)?;
    open_private_service_log(Path::new(OPENRC_STDERR_LOG), 0o640)?;
    migrate_service_log_permissions()?;

    eprintln!("  Generating OpenRC init script...");
    eprintln!("    Binary:  {}", exe_str);
    eprintln!("    Config:  {}", config_str);
    eprintln!("    WorkDir: {}", working_dir);

    let init_content = render_openrc_init(exe_str, config_str, working_dir);
    write_service_definition(OPENRC_INIT_PATH, &init_content, 0o755)?;

    eprintln!("  Enabling and starting service...");
    run_cmd(openrc_update_bin(), &["add", SERVICE_NAME, "default"])?;
    run_cmd(openrc_service_bin(), &[SERVICE_NAME, "start"])?;

    eprintln!();
    if manager_is_active(ServiceManager::OpenRc) {
        eprintln!("  Service started successfully!");
    } else {
        eprintln!("  Service state is not active yet. Check `sudo ./aether-tunnel logs`.");
    }

    print_post_install_commands();
    Ok(())
}

fn render_openrc_init(exe_str: &str, config_str: &str, working_dir: &str) -> String {
    format!(
        r#"#!{}
name={}
description={}
supervisor=supervise-daemon
command={}
directory={}
pidfile={}
output_log_dir={}
output_log={}
error_log={}
supervise_daemon={}
config_env={}
service_manager_env={}
log_destination_env={}
log_dir_env={}
respawn_delay=5
respawn_max=10
respawn_period=60

depend() {{
    after net
}}

start_pre() {{
    checkpath --directory --mode 0750 "$output_log_dir"
    checkpath --file --mode 0640 "$output_log"
    checkpath --file --mode 0640 "$error_log"
}}

start() {{
    ebegin "Starting ${{RC_SVCNAME}}"
    "$supervise_daemon" "${{RC_SVCNAME}}" \
        --start "$command" \
        --pidfile "$pidfile" \
        --chdir "$directory" \
        --stdout "$output_log" \
        --stderr "$error_log" \
        --user "{SERVICE_USER}:{SERVICE_GROUP}" \
        --respawn-delay "$respawn_delay" \
        --respawn-max "$respawn_max" \
        --respawn-period "$respawn_period" \
        --umask 0077 \
        --env "$config_env" \
        --env "$service_manager_env" \
        --env "$log_destination_env" \
        --env "$log_dir_env"
    eend $?
}}

stop() {{
    ebegin "Stopping ${{RC_SVCNAME}}"
    "$supervise_daemon" "${{RC_SVCNAME}}" --stop "$command" --pidfile "$pidfile"
    eend $?
}}
"#,
        openrc_run_bin(),
        shell_quote(SERVICE_NAME),
        shell_quote("Aether Tunnel"),
        shell_quote(exe_str),
        shell_quote(working_dir),
        shell_quote(OPENRC_PID_PATH),
        shell_quote(OPENRC_LOG_DIR),
        shell_quote(OPENRC_STDOUT_LOG),
        shell_quote(OPENRC_STDERR_LOG),
        shell_quote(supervise_daemon_bin()),
        shell_quote(&format!("AETHER_TUNNEL_CONFIG={config_str}")),
        shell_quote("AETHER_TUNNEL_SERVICE_MANAGER=openrc"),
        shell_quote("AETHER_TUNNEL_LOG_DESTINATION=both"),
        shell_quote(&format!("AETHER_TUNNEL_LOG_DIR={OPENRC_LOG_DIR}")),
    )
}

fn uninstall_systemd_service() -> anyhow::Result<()> {
    eprintln!("  Stopping and removing existing service...");
    let _ = Command::new(systemctl_bin())
        .args(["disable", "--now", SERVICE_NAME])
        .status();

    if Path::new(SYSTEMD_UNIT_PATH).exists() {
        std::fs::remove_file(SYSTEMD_UNIT_PATH)?;
        eprintln!("  Removed {}", SYSTEMD_UNIT_PATH);
    }

    run_cmd(systemctl_bin(), &["daemon-reload"])?;
    eprintln!("  Service uninstalled.");
    Ok(())
}

fn uninstall_openrc_service() -> anyhow::Result<()> {
    eprintln!("  Stopping and removing existing service...");
    let _ = Command::new(openrc_service_bin())
        .args([SERVICE_NAME, "stop"])
        .status();
    let _ = Command::new(openrc_update_bin())
        .args(["del", SERVICE_NAME, "default"])
        .status();

    if Path::new(OPENRC_INIT_PATH).exists() {
        std::fs::remove_file(OPENRC_INIT_PATH)?;
        eprintln!("  Removed {}", OPENRC_INIT_PATH);
    }

    eprintln!("  Service uninstalled.");
    Ok(())
}

fn start_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd(systemctl_bin(), &["start", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "start"]),
    }
}

fn stop_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd(systemctl_bin(), &["stop", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "stop"]),
    }
}

fn restart_manager(manager: ServiceManager) -> anyhow::Result<()> {
    match manager {
        ServiceManager::Systemd => run_cmd(systemctl_bin(), &["restart", SERVICE_NAME]),
        ServiceManager::OpenRc => run_cmd(openrc_service_bin(), &[SERVICE_NAME, "restart"]),
    }
}

fn manager_status(manager: ServiceManager) -> anyhow::Result<ExitStatus> {
    let status = match manager {
        ServiceManager::Systemd => Command::new(systemctl_bin())
            .args(["status", SERVICE_NAME])
            .status()?,
        ServiceManager::OpenRc => Command::new(openrc_service_bin())
            .args([SERVICE_NAME, "status"])
            .status()?,
    };
    Ok(status)
}

fn manager_is_active(manager: ServiceManager) -> bool {
    match manager {
        ServiceManager::Systemd => {
            Path::new(SYSTEMD_UNIT_PATH).exists()
                && Command::new(systemctl_bin())
                    .args(["is-active", "--quiet", SERVICE_NAME])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
        }
        ServiceManager::OpenRc => {
            Path::new(OPENRC_INIT_PATH).exists()
                && Command::new(openrc_service_bin())
                    .args([SERVICE_NAME, "status"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
        }
    }
}

fn print_post_install_commands() {
    eprintln!();
    eprintln!("  Commands:");
    eprintln!("    ./aether-tunnel status          # service status");
    eprintln!("    sudo ./aether-tunnel logs       # tail logs");
    eprintln!("    sudo ./aether-tunnel restart    # restart");
    eprintln!("    sudo ./aether-tunnel stop       # stop");
    eprintln!("    sudo ./aether-tunnel uninstall  # remove service");
    eprintln!();
}

fn is_systemd_available() -> bool {
    Path::new("/run/systemd/system").exists()
        && has_absolute_candidate(SYSTEMCTL_BINS)
        && Command::new(systemctl_bin())
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
}

fn is_openrc_available() -> bool {
    (Path::new("/run/openrc").exists() || Path::new("/run/openrc/softlevel").exists())
        && has_absolute_candidate(OPENRC_RUN_BINS)
        && has_absolute_candidate(OPENRC_SERVICE_BINS)
        && has_absolute_candidate(OPENRC_UPDATE_BINS)
        && has_absolute_candidate(OPENRC_SUPERVISE_BINS)
}

fn has_absolute_candidate(candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.starts_with('/') && Path::new(candidate).exists())
}

fn openrc_run_bin() -> &'static str {
    pick_bin(OPENRC_RUN_BINS)
}

fn openrc_service_bin() -> &'static str {
    pick_bin(OPENRC_SERVICE_BINS)
}

fn openrc_update_bin() -> &'static str {
    pick_bin(OPENRC_UPDATE_BINS)
}

fn supervise_daemon_bin() -> &'static str {
    pick_bin(OPENRC_SUPERVISE_BINS)
}

fn tail_bin() -> &'static str {
    pick_bin(TAIL_BINS)
}

fn pick_bin(candidates: &[&'static str]) -> &'static str {
    candidates
        .iter()
        .copied()
        .find(|candidate| candidate.starts_with('/') && Path::new(candidate).exists())
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .find(|candidate| candidate.starts_with('/'))
        })
        .expect("trusted binary candidate list must include an absolute path")
}

fn id_bin() -> &'static str {
    pick_bin(ID_BINS)
}
fn groupadd_bin() -> &'static str {
    pick_bin(GROUPADD_BINS)
}
fn useradd_bin() -> &'static str {
    pick_bin(USERADD_BINS)
}
fn addgroup_bin() -> &'static str {
    pick_bin(ADDGROUP_BINS)
}
fn adduser_bin() -> &'static str {
    pick_bin(ADDUSER_BINS)
}
fn chown_bin() -> &'static str {
    pick_bin(CHOWN_BINS)
}
fn getent_bin() -> &'static str {
    pick_bin(GETENT_BINS)
}

fn systemctl_bin() -> &'static str {
    pick_bin(SYSTEMCTL_BINS)
}

fn journalctl_bin() -> &'static str {
    pick_bin(JOURNALCTL_BINS)
}

fn validate_service_unit_path(value: &str, label: &str) -> anyhow::Result<()> {
    // systemd/OpenRC consume POSIX paths even when this module is compiled on
    // another host (the renderer is also covered by cross-platform tests).
    // `Path::is_absolute()` follows the build host's path syntax and would
    // reject valid `/opt/...` service paths on Windows.
    if !value.starts_with('/') {
        anyhow::bail!("{} path must be absolute", label);
    }
    if value.chars().any(char::is_control) || value.contains(['%', '$']) {
        anyhow::bail!(
            "{} path contains control or service-manager expansion characters",
            label
        );
    }
    Ok(())
}

/// Check the supplied path without resolving symlinks or collapsing `..`.
#[cfg(unix)]
fn prepare_service_config_path(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    open_service_config(&absolute)?;
    Ok(absolute)
}

#[cfg(unix)]
struct ServiceConfigFile {
    ancestors: Vec<std::fs::File>,
    parent: std::fs::File,
    name: std::ffi::CString,
    source: std::fs::File,
}

#[cfg(unix)]
fn open_config_entry(
    parent: &std::fs::File,
    name: &std::ffi::CStr,
    flags: i32,
    mode: libc::mode_t,
) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // Every component is opened relative to a checked directory FD. Nonblocking
    // prevents a swapped FIFO from hanging before its file type can be checked.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }
}

#[cfg(unix)]
fn check_config_directory(directory: &std::fs::File, owner: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
        anyhow::bail!("service config ancestors must be root-owned and not group/other writable (including sticky directories)");
    }
    Ok(())
}

#[cfg(unix)]
fn check_config_source(source: &std::fs::File, owner: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = source.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != owner
        || metadata.mode() & 0o022 != 0
    {
        anyhow::bail!(
            "service config must be a root-owned, non-writable, single-link regular file"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn open_service_config_at(
    mut directory: std::fs::File,
    relative: &Path,
    owner: u32,
) -> anyhow::Result<ServiceConfigFile> {
    use std::os::unix::ffi::OsStrExt;
    let mut names = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(name) => {
                names.push(std::ffi::CString::new(name.as_bytes())?)
            }
            std::path::Component::CurDir => {}
            _ => anyhow::bail!("service config path must not contain '..' or a path prefix"),
        }
    }
    let name = names
        .pop()
        .ok_or_else(|| anyhow::anyhow!("service config has no file name"))?;
    check_config_directory(&directory, owner)?;
    let mut ancestors = Vec::new();
    for component in names {
        let next = open_config_entry(
            &directory,
            &component,
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?;
        ancestors.push(directory);
        directory = next;
        check_config_directory(&directory, owner)?;
    }
    let source = open_config_entry(&directory, &name, libc::O_RDONLY, 0)?;
    check_config_source(&source, owner)?;
    Ok(ServiceConfigFile {
        ancestors,
        parent: directory,
        name,
        source,
    })
}

#[cfg(unix)]
fn open_service_config(path: &Path) -> anyhow::Result<ServiceConfigFile> {
    use std::os::unix::fs::OpenOptionsExt;
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    open_service_config_at(root, path.strip_prefix("/")?, 0)
}

#[cfg(not(unix))]
fn prepare_service_config_path(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    let _ = path;
    anyhow::bail!("managed tunnel services require Unix filesystem checks")
}

fn validate_root_managed_service_file(
    path: &Path,
    label: &str,
    require_private_file: bool,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let file_metadata = std::fs::symlink_metadata(path)?;
        if !file_metadata.is_file() || file_metadata.file_type().is_symlink() {
            anyhow::bail!("service {} must be a regular non-symlink file", label);
        }
        if file_metadata.uid() != 0 || file_metadata.mode() & 0o022 != 0 {
            anyhow::bail!(
                "service {} must be owned by root and not writable by group or other users; use the official installer or move it to a root-managed path",
                label
            );
        }
        if require_private_file && (file_metadata.mode() & 0o077 != 0 || file_metadata.nlink() != 1)
        {
            anyhow::bail!(
                "service {} contains credentials and must be owner-only with exactly one hard link",
                label
            );
        }

        let mut ancestor = path.parent();
        while let Some(directory) = ancestor {
            let metadata = std::fs::symlink_metadata(directory)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
            {
                anyhow::bail!(
                    "service {} parent '{}' must be a root-owned directory that is not writable by group or other users",
                    label,
                    directory.display()
                );
            }
            ancestor = directory.parent();
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (path, label, require_private_file);
        anyhow::bail!("managed tunnel services require Unix ownership checks")
    }
}

fn validate_service_config_file(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.uid() != 0 {
            anyhow::bail!("service config must be a regular root-owned file");
        }
        if metadata.nlink() != 1 || metadata.mode() & 0o777 != 0o640 {
            anyhow::bail!("service config must be root-owned, single-link, and mode 0640");
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        anyhow::bail!("managed tunnel services require Unix ownership checks")
    }
}

fn ensure_service_identity() -> anyhow::Result<()> {
    if !is_root() {
        anyhow::bail!("root required to create the tunnel service identity");
    }
    let group = Command::new(getent_bin())
        .args(["group", SERVICE_GROUP])
        .output()?;
    if !group.status.success() {
        let status = if Path::new(groupadd_bin()).exists() {
            Command::new(groupadd_bin())
                .args(["--system", SERVICE_GROUP])
                .status()?
        } else {
            Command::new(addgroup_bin())
                .args(["-S", SERVICE_GROUP])
                .status()?
        };
        if !status.success() {
            anyhow::bail!("failed to create service group '{SERVICE_GROUP}'");
        }
    }
    let user = Command::new(getent_bin())
        .args(["passwd", SERVICE_USER])
        .output()?;
    if !user.status.success() {
        let status = if Path::new(useradd_bin()).exists() {
            Command::new(useradd_bin())
                .args([
                    "--system",
                    "--no-create-home",
                    "--shell",
                    "/usr/sbin/nologin",
                    "--gid",
                    SERVICE_GROUP,
                    SERVICE_USER,
                ])
                .status()?
        } else {
            Command::new(adduser_bin())
                .args([
                    "-S",
                    "-D",
                    "-H",
                    "-G",
                    SERVICE_GROUP,
                    "-s",
                    "/sbin/nologin",
                    SERVICE_USER,
                ])
                .status()?
        };
        if !status.success() {
            anyhow::bail!("failed to create service user '{SERVICE_USER}'");
        }
    }
    let uid = Command::new(id_bin()).args(["-u", SERVICE_USER]).output()?;
    let gid = Command::new(id_bin()).args(["-g", SERVICE_USER]).output()?;
    validate_service_uid(uid.status.success(), &uid.stdout)?;
    if !gid.status.success()
        || String::from_utf8_lossy(&gid.stdout).trim() != service_group_id()?.to_string()
    {
        anyhow::bail!("service identity '{SERVICE_USER}' must use primary group '{SERVICE_GROUP}'");
    }
    let groups = Command::new(id_bin()).args(["-G", SERVICE_USER]).output()?;
    let expected_gid = service_group_id()?.to_string();
    if !groups.status.success()
        || String::from_utf8_lossy(&groups.stdout)
            .split_whitespace()
            .any(|group| group != expected_gid)
    {
        anyhow::bail!("service identity '{SERVICE_USER}' has unexpected supplementary groups");
    }
    Ok(())
}

fn validate_service_uid(status_success: bool, output: &[u8]) -> anyhow::Result<()> {
    if !status_success {
        anyhow::bail!("cannot resolve service identity '{SERVICE_USER}' uid");
    }
    let uid = String::from_utf8_lossy(output)
        .trim()
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("cannot parse service identity '{SERVICE_USER}' uid"))?;
    if uid == 0 {
        anyhow::bail!("service identity '{SERVICE_USER}' must not use uid 0");
    }
    Ok(())
}

fn service_group_id() -> anyhow::Result<u32> {
    let output = Command::new(id_bin())
        .args(["-g", SERVICE_GROUP])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("cannot resolve service group");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().parse()?)
}

fn migrate_service_permissions(config: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        // Only the documented dedicated config directory may be migrated.
        // Other root-managed directories keep their ownership and permissions.
        let migrate_parent = config.parent() == Some(Path::new("/etc/aether-tunnel"));
        migrate_service_config(
            open_service_config(config)?,
            0,
            service_group_id()?,
            migrate_parent,
        )
    }

    #[cfg(not(unix))]
    {
        let _ = config;
        anyhow::bail!("managed tunnel services require Unix filesystem checks")
    }
}

#[cfg(unix)]
fn migrate_service_config(
    mut config: ServiceConfigFile,
    owner: u32,
    group: u32,
    migrate_parent: bool,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::io::{self, Read};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    for directory in config
        .ancestors
        .iter()
        .chain(std::iter::once(&config.parent))
    {
        check_config_directory(directory, owner)?;
    }
    // No root-owned source inode is ever chmod/chowned: all permission changes
    // apply to a new private inode. Root is trusted; concurrent root setup/save
    // is unsupported (the final identity check is detection, not a rename CAS).
    check_config_source(&config.source, owner)?;
    let original = config.source.metadata()?;
    let max_bytes = crate::config::MAX_CONFIG_FILE_BYTES;
    if original.len() > max_bytes {
        anyhow::bail!("service config exceeds the {max_bytes} byte limit");
    }
    for directory in config
        .ancestors
        .iter()
        .chain((!migrate_parent).then_some(&config.parent))
    {
        let metadata = directory.metadata()?;
        if metadata.mode() & 0o001 == 0
            && !(metadata.gid() == group && metadata.mode() & 0o010 != 0)
        {
            anyhow::bail!("service account cannot traverse config directory; use /etc/aether-tunnel or provision custom directory access explicitly");
        }
    }

    let temporary_name = std::ffi::CString::new(format!(
        ".aether-tunnel-config-{}.tmp",
        uuid::Uuid::new_v4()
    ))?;
    let mut temporary = open_config_entry(
        &config.parent,
        &temporary_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        0o600,
    )?;
    let mut committed = false;
    let result = (|| -> anyhow::Result<()> {
        let copied = io::copy(
            &mut Read::by_ref(&mut config.source).take(max_bytes + 1),
            &mut temporary,
        )?;
        if copied > max_bytes {
            anyhow::bail!("service config exceeds the {max_bytes} byte limit");
        }
        let current = open_config_entry(&config.parent, &config.name, libc::O_RDONLY, 0)?;
        check_config_source(&current, owner)?;
        let metadata = current.metadata()?;
        if metadata.dev() != original.dev()
            || metadata.ino() != original.ino()
            || metadata.len() != original.len()
            || copied != original.len()
            || metadata.mtime() != original.mtime()
            || metadata.mtime_nsec() != original.mtime_nsec()
            || metadata.ctime() != original.ctime()
            || metadata.ctime_nsec() != original.ctime_nsec()
        {
            anyhow::bail!("service config changed during migration; refusing replacement");
        }
        if unsafe { libc::fchown(temporary.as_raw_fd(), owner, group) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        temporary.set_permissions(std::fs::Permissions::from_mode(0o640))?;
        temporary.sync_all()?;
        if unsafe {
            libc::renameat(
                config.parent.as_raw_fd(),
                temporary_name.as_ptr(),
                config.parent.as_raw_fd(),
                config.name.as_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error().into());
        }
        committed = true;
        // The rename is the commit point. Errors after it explicitly report
        // partial completion; no unsafe path-based rollback is attempted.
        if migrate_parent {
            if unsafe { libc::fchown(config.parent.as_raw_fd(), owner, group) } != 0 {
                return Err(io::Error::last_os_error()).context("config replaced, but dedicated directory ownership migration failed; service installation aborted");
            }
            config.parent.set_permissions(std::fs::Permissions::from_mode(0o750))
                .context("config replaced, but dedicated directory mode migration failed; service installation aborted")?;
        }
        config.parent.sync_all().context("config replaced, but directory durability is unconfirmed; service installation aborted")?;
        Ok(())
    })();
    if result.is_err() && !committed {
        // Reduce exposure if cleanup itself fails; report both failures rather
        // than silently leaving a credential-bearing temporary file behind.
        let restrict = temporary.set_permissions(std::fs::Permissions::from_mode(0o600));
        if unsafe { libc::unlinkat(config.parent.as_raw_fd(), temporary_name.as_ptr(), 0) } != 0 {
            let cleanup = io::Error::last_os_error();
            return result.context(format!(
                "temporary config cleanup failed: {cleanup}; owner-only restriction: {restrict:?}"
            ));
        }
    }
    result
}

fn migrate_service_log_permissions() -> anyhow::Result<()> {
    run_cmd(
        chown_bin(),
        &[&format!("{SERVICE_USER}:{SERVICE_GROUP}"), OPENRC_LOG_DIR],
    )?;
    for path in [OPENRC_STDOUT_LOG, OPENRC_STDERR_LOG] {
        if Path::new(path).exists() {
            run_cmd(
                chown_bin(),
                &[&format!("{SERVICE_USER}:{SERVICE_GROUP}"), path],
            )?;
        }
    }
    Ok(())
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

fn write_service_definition(path: &str, content: &str, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let requested_path = Path::new(path);
        let requested_parent = requested_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("service definition path has no parent"))?;
        let file_name = requested_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("service definition path has no file name"))?;
        let parent = std::fs::canonicalize(requested_parent)?;
        let path = parent.join(file_name);
        validate_private_service_directory(&parent)?;
        validate_replaceable_service_file(&path)?;

        let temporary = parent.join(format!(
            ".aether-tunnel-service-{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut file = options.open(&temporary)?;
        let result = (|| -> anyhow::Result<()> {
            // SAFETY: geteuid has no preconditions and does not retain pointers.
            let effective_uid = unsafe { libc::geteuid() };
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.uid() != effective_uid || metadata.nlink() != 1 {
                anyhow::bail!("temporary service definition has unsafe ownership or links");
            }
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &path)?;
            std::fs::File::open(&parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    #[cfg(not(unix))]
    {
        let _ = (path, content, mode);
        anyhow::bail!("managed service definitions require Unix filesystem checks")
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn validate_private_service_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || (metadata.uid() != effective_uid && metadata.uid() != 0)
            || metadata.mode() & 0o022 != 0
        {
            anyhow::bail!(
                "service directory '{}' has unsafe ownership or permissions",
                path.display()
            );
        }

        let canonical = std::fs::canonicalize(path)?;
        let mut ancestor = canonical.parent();
        while let Some(directory) = ancestor {
            let metadata = std::fs::symlink_metadata(directory)?;
            let mode = metadata.mode();
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || (metadata.uid() != effective_uid && metadata.uid() != 0)
                || (mode & 0o022 != 0 && mode & 0o1000 == 0)
            {
                anyhow::bail!(
                    "service directory ancestor '{}' has unsafe ownership or permissions",
                    directory.display()
                );
            }
            ancestor = directory.parent();
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        anyhow::bail!("managed service directories require Unix filesystem checks")
    }
}

fn validate_replaceable_service_file(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                // SAFETY: geteuid has no preconditions and does not retain pointers.
                let effective_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != effective_uid
                    || metadata.nlink() != 1
                {
                    anyhow::bail!(
                        "service file '{}' must be a regular single-link file owned by the current user",
                        path.display()
                    );
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        anyhow::bail!("managed service files require Unix filesystem checks")
    }
}

fn ensure_private_service_directory(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

        let requested_parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("service directory has no parent"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("service directory has no file name"))?;
        let parent = std::fs::canonicalize(requested_parent)?;
        let path = parent.join(file_name);
        validate_private_service_directory(&parent)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let mut builder = std::fs::DirBuilder::new();
                builder.mode(mode).create(&path)?;
            }
            Err(error) => return Err(error.into()),
        }

        let mut options = OpenOptions::new();
        options.read(true).custom_flags(
            libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK,
        );
        let directory = options.open(&path)?;
        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.uid() != effective_uid || metadata.mode() & 0o022 != 0 {
            anyhow::bail!("service log directory has unsafe ownership or permissions");
        }
        directory.set_permissions(std::fs::Permissions::from_mode(mode))?;
        directory.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        anyhow::bail!("managed service directories require Unix filesystem checks")
    }
}

fn open_private_service_log(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let requested_parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("service log path has no parent"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("service log path has no file name"))?;
        let parent = std::fs::canonicalize(requested_parent)?;
        let path = parent.join(file_name);
        validate_private_service_directory(&parent)?;
        let mut options = OpenOptions::new();
        options
            .create(true)
            .append(true)
            .mode(mode)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = options.open(&path)?;
        // SAFETY: geteuid has no preconditions and does not retain pointers.
        let effective_uid = unsafe { libc::geteuid() };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != effective_uid || metadata.nlink() != 1 {
            anyhow::bail!("service log must be a regular, single-link file owned by root");
        }
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        anyhow::bail!("managed service logs require Unix filesystem checks")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_private_service_directory, open_private_service_log, pick_bin, render_openrc_init,
        render_systemd_unit, systemd_quote, validate_root_managed_service_file,
        validate_service_uid, validate_service_unit_path, write_service_definition,
    };

    #[test]
    fn systemd_unit_quotes_paths_without_changing_arguments() {
        let unit = render_systemd_unit(
            r#"/opt/Aether Tunnel/aether\"tunnel"#,
            r#"/var/lib/aether tunnel/config\\node.toml"#,
            "/var/lib/aether tunnel",
        )
        .expect("safe absolute paths should render");

        assert!(unit.contains(r#"ExecStart="/opt/Aether Tunnel/aether\\\"tunnel""#));
        assert!(unit.contains(
            r#"Environment="AETHER_TUNNEL_CONFIG=/var/lib/aether tunnel/config\\\\node.toml""#
        ));
        assert!(unit.contains(r#"WorkingDirectory="/var/lib/aether tunnel""#));
        assert_eq!(systemd_quote("a\\b\"c"), r#""a\\b\"c""#);
    }

    #[test]
    fn service_renderers_drop_privileges() {
        let unit = render_systemd_unit(
            "/usr/local/bin/aether-tunnel",
            "/etc/aether-tunnel/aether-tunnel.toml",
            "/etc/aether-tunnel",
        )
        .unwrap();
        assert!(unit.contains("User=aether-tunnel\n"));
        assert!(unit.contains("Group=aether-tunnel\n"));
        assert!(unit.contains("NoNewPrivileges=true\n"));
        assert!(unit.contains("CapabilityBoundingSet=\n"));
        assert!(!unit.contains("User=root"));

        let init = render_openrc_init(
            "/usr/local/bin/aether-tunnel",
            "/etc/aether-tunnel/aether-tunnel.toml",
            "/etc/aether-tunnel",
        );
        assert!(init.contains("--user \"aether-tunnel:aether-tunnel\""));
        assert!(!init.contains("--user root"));
    }

    #[test]
    fn service_identity_uid_fixture_rejects_root() {
        let fixtures = [
            (b"1001\n".as_slice(), true),
            (b"0\n".as_slice(), false),
            (b"00\n".as_slice(), false),
        ];
        for (uid, accepted) in fixtures {
            assert_eq!(validate_service_uid(true, uid).is_ok(), accepted);
        }
        assert!(validate_service_uid(false, b"1001\n").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn service_config_fd_walk_rejects_links_and_ambiguous_paths_without_mutation() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

        let fixture = ConfigFixture::new();
        let directory = &fixture.path;

        let target = directory.join("target.toml");
        std::fs::write(&target, b"sentinel").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let target_before = std::fs::symlink_metadata(&target).unwrap();

        let symlink_path = directory.join("symlink.toml");
        symlink(&target, &symlink_path).unwrap();
        assert!(fixture.open(Path::new("symlink.toml")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
        let target_after_symlink = std::fs::symlink_metadata(&target).unwrap();
        assert_eq!(target_after_symlink.ino(), target_before.ino());
        assert_eq!(target_after_symlink.mode() & 0o777, 0o644);

        let hardlink_path = directory.join("hardlink.toml");
        std::fs::hard_link(&target, &hardlink_path).unwrap();
        assert!(fixture.open(Path::new("hardlink.toml")).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"sentinel");
        assert_eq!(
            std::fs::symlink_metadata(&target).unwrap().nlink(),
            2,
            "preflight must not unlink or rewrite a hard-linked target"
        );

        let real_directory = directory.join("real");
        std::fs::create_dir(&real_directory).unwrap();
        std::fs::set_permissions(&real_directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let nested_target = real_directory.join("nested.toml");
        std::fs::write(&nested_target, b"nested sentinel").unwrap();
        let linked_directory = directory.join("linked");
        symlink(&real_directory, &linked_directory).unwrap();
        assert!(fixture.open(Path::new("linked/nested.toml")).is_err());
        assert!(fixture
            .open(Path::new("linked/../real/nested.toml"))
            .is_err());
        assert_eq!(std::fs::read(&nested_target).unwrap(), b"nested sentinel");
        assert!(
            fixture.open(Path::new("real/nested.toml")).is_ok(),
            "the same fixture must accept the actual safe path"
        );
    }

    #[cfg(unix)]
    use std::path::{Path, PathBuf};

    #[cfg(unix)]
    struct ConfigFixture {
        path: PathBuf,
    }

    #[cfg(unix)]
    impl ConfigFixture {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            // Test the exact FD walker from a private fixture root. Production
            // always starts at / and requires uid 0; no test weakens that rule
            // or relies on a rejection from macOS's common /tmp symlink.
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("aether-config-migration-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { path }
        }

        fn open(&self, relative: &Path) -> anyhow::Result<super::ServiceConfigFile> {
            super::open_service_config_at(std::fs::File::open(&self.path)?, relative, unsafe {
                libc::geteuid()
            })
        }

        fn config(&self, parent: &str, mode: u32) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let directory = self.path.join(parent);
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(mode)).unwrap();
            let path = directory.join("config.toml");
            std::fs::write(&path, b"management_token = 'fixture-only'\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            Path::new(parent).join("config.toml")
        }

        fn migrate(&self, relative: &Path, dedicated: bool) -> anyhow::Result<()> {
            super::migrate_service_config(
                self.open(relative)?,
                unsafe { libc::geteuid() },
                unsafe { libc::getegid() },
                dedicated,
            )
        }
    }

    #[cfg(unix)]
    impl Drop for ConfigFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn service_config_migration_preserves_contents_and_source_inode() {
        use std::os::unix::fs::MetadataExt;
        let fixture = ConfigFixture::new();
        let relative = fixture.config("dedicated", 0o700);
        let path = fixture.path.join(&relative);
        let source = std::fs::File::open(&path).unwrap();
        let original = source.metadata().unwrap();
        fixture.migrate(&relative, true).unwrap();
        let migrated = std::fs::metadata(&path).unwrap();
        assert_ne!(migrated.ino(), original.ino());
        assert_eq!(source.metadata().unwrap().mode(), original.mode());
        assert_eq!(migrated.uid(), unsafe { libc::geteuid() });
        assert_eq!(migrated.gid(), unsafe { libc::getegid() });
        assert_eq!(migrated.mode() & 0o777, 0o640);
        assert_eq!(migrated.nlink(), 1);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"management_token = 'fixture-only'\n"
        );
        let parent = std::fs::metadata(path.parent().unwrap()).unwrap();
        assert_eq!(parent.mode() & 0o777, 0o750);
        assert_eq!(parent.gid(), unsafe { libc::getegid() });
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        fixture.migrate(&relative, true).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn service_config_migration_preserves_custom_parent_and_rejects_sticky_parent() {
        use std::os::unix::fs::MetadataExt;
        let fixture = ConfigFixture::new();
        let custom = fixture.config("custom", 0o755);
        let parent = fixture.path.join("custom");
        let before = std::fs::metadata(&parent).unwrap();
        fixture.migrate(&custom, false).unwrap();
        let after = std::fs::metadata(&parent).unwrap();
        assert_eq!(
            (after.uid(), after.gid(), after.mode()),
            (before.uid(), before.gid(), before.mode())
        );
        let sticky = fixture.config("sticky", 0o1777);
        let before = std::fs::metadata(fixture.path.join("sticky")).unwrap();
        let source_before = std::fs::metadata(fixture.path.join(&sticky)).unwrap();
        assert!(fixture.migrate(&sticky, true).is_err());
        let after = std::fs::metadata(fixture.path.join("sticky")).unwrap();
        let source_after = std::fs::metadata(fixture.path.join(&sticky)).unwrap();
        assert_eq!(
            (after.uid(), after.gid(), after.mode()),
            (before.uid(), before.gid(), before.mode())
        );
        assert_eq!(
            (source_before.ino(), source_before.mode()),
            (source_after.ino(), source_after.mode())
        );
        assert_eq!(
            std::fs::read_dir(fixture.path.join("sticky"))
                .unwrap()
                .count(),
            1
        );
        let private = fixture.config("private-custom", 0o700);
        assert!(fixture
            .migrate(&private, false)
            .unwrap_err()
            .to_string()
            .contains("cannot traverse"));
    }

    #[cfg(unix)]
    #[test]
    fn service_config_migration_stays_in_pinned_directory_after_ancestor_swap() {
        use std::os::unix::fs::symlink;
        let fixture = ConfigFixture::new();
        let original = fixture.config("original", 0o700);
        let target = fixture.config("target", 0o700);
        std::fs::write(fixture.path.join(&target), b"unrelated target").unwrap();
        let opened = fixture.open(&original).unwrap();
        std::fs::rename(fixture.path.join("original"), fixture.path.join("moved")).unwrap();
        symlink(fixture.path.join("target"), fixture.path.join("original")).unwrap();
        super::migrate_service_config(
            opened,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
            true,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(fixture.path.join(&target)).unwrap(),
            b"unrelated target"
        );
        assert_eq!(
            std::fs::read(fixture.path.join("moved/config.toml")).unwrap(),
            b"management_token = 'fixture-only'\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn service_config_migration_rejects_fifo_and_changed_entry_and_cleans_temporary_file() {
        use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
        let fixture = ConfigFixture::new();
        let path = fixture.config("dedicated", 0o700);
        let opened = fixture.open(&path).unwrap();
        let parent_before = std::fs::metadata(fixture.path.join("dedicated")).unwrap();
        let actual = fixture.path.join(&path);
        std::fs::rename(&actual, actual.with_extension("old")).unwrap();
        let name = std::ffi::CString::new(actual.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(fixture.open(&path).is_err());
        let error = super::migrate_service_config(
            opened,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("regular file"));
        let parent_after = std::fs::metadata(fixture.path.join("dedicated")).unwrap();
        assert_eq!(parent_before.mode(), parent_after.mode());
        assert_eq!(
            std::fs::read_dir(fixture.path.join("dedicated"))
                .unwrap()
                .count(),
            2,
            "failed migration must remove its private temp file"
        );
        std::fs::remove_file(&actual).unwrap();
        std::fs::write(&actual, b"replacement").unwrap();
        let opened = fixture.open(&path).unwrap();
        std::fs::rename(&actual, actual.with_extension("new")).unwrap();
        std::fs::write(&actual, b"replacement").unwrap();
        assert!(super::migrate_service_config(
            opened,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
            true
        )
        .unwrap_err()
        .to_string()
        .contains("changed during migration"));
    }

    #[cfg(unix)]
    #[test]
    fn service_config_migration_rejects_oversized_source_before_mutation() {
        use std::os::unix::fs::MetadataExt;
        let fixture = ConfigFixture::new();
        let path = fixture.config("dedicated", 0o700);
        let actual = fixture.path.join(&path);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&actual)
            .unwrap();
        file.set_len(crate::config::MAX_CONFIG_FILE_BYTES + 1)
            .unwrap();
        let before = file.metadata().unwrap();
        assert!(fixture
            .migrate(&path, true)
            .unwrap_err()
            .to_string()
            .contains("byte limit"));
        let after = std::fs::metadata(&actual).unwrap();
        assert_eq!((before.ino(), before.mode()), (after.ino(), after.mode()));
        assert_eq!(
            std::fs::read_dir(fixture.path.join("dedicated"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn service_unit_paths_reject_directive_and_expansion_injection() {
        for value in [
            "relative/path",
            "/tmp/config\nExecStart=/tmp/evil",
            "/tmp/config\rEnvironment=EVIL=1",
            "/tmp/%n/config",
            "/tmp/$PATH/config",
        ] {
            assert!(
                validate_service_unit_path(value, "test").is_err(),
                "accepted unsafe service path: {value:?}"
            );
        }
    }

    #[test]
    fn service_commands_never_fall_back_to_path_lookup() {
        assert_eq!(
            pick_bin(&["relative-tool", "/definitely/missing/trusted-tool"]),
            "/definitely/missing/trusted-tool"
        );
    }

    #[cfg(unix)]
    #[test]
    fn root_service_rejects_files_beneath_shared_writable_directories() {
        let directory =
            std::env::temp_dir().join(format!("aether-service-path-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).expect("test directory should be created");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o777))
                .expect("test directory permissions should be set");
        }
        let path = directory.join("aether-tunnel");
        std::fs::write(&path, b"binary").expect("test binary should be written");

        let result = validate_root_managed_service_file(&path, "binary", false);
        let _ = std::fs::remove_dir_all(&directory);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn service_definitions_and_logs_refuse_links_and_use_private_atomic_files() {
        use std::io::Read;
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

        let directory = std::env::temp_dir().join(format!(
            "aether-service-write-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();

        let definition = directory.join("aether-tunnel.service");
        write_service_definition(definition.to_str().unwrap(), "first", 0o644).unwrap();
        let metadata = std::fs::symlink_metadata(&definition).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o644);
        assert_eq!(metadata.nlink(), 1);
        let mut old_definition = std::fs::File::open(&definition).unwrap();
        write_service_definition(definition.to_str().unwrap(), "second", 0o644).unwrap();
        let mut old_contents = String::new();
        old_definition.read_to_string(&mut old_contents).unwrap();
        assert_eq!(old_contents, "first");
        assert_eq!(std::fs::read_to_string(&definition).unwrap(), "second");

        let victim = directory.join("victim");
        std::fs::write(&victim, b"known-good").unwrap();
        std::fs::remove_file(&definition).unwrap();
        symlink(&victim, &definition).unwrap();
        assert!(write_service_definition(definition.to_str().unwrap(), "replace", 0o644).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_file(&definition).unwrap();
        std::fs::hard_link(&victim, &definition).unwrap();
        assert!(write_service_definition(definition.to_str().unwrap(), "replace", 0o644).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");
        std::fs::remove_file(&definition).unwrap();

        let log_directory = directory.join("logs");
        ensure_private_service_directory(&log_directory, 0o750).unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&log_directory).unwrap().mode() & 0o777,
            0o750
        );
        let log = log_directory.join("current.log");
        open_private_service_log(&log, 0o640).unwrap();
        let metadata = std::fs::symlink_metadata(&log).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o640);
        assert_eq!(metadata.nlink(), 1);
        std::fs::remove_file(&log).unwrap();
        symlink(&victim, &log).unwrap();
        assert!(open_private_service_log(&log, 0o640).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_file(&log).unwrap();
        std::fs::hard_link(&victim, &log).unwrap();
        assert!(open_private_service_log(&log, 0o640).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_dir_all(directory).unwrap();
    }
}
