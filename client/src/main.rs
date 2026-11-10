use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use dog_device_client::device_id::{self, CandidateStatus, DeviceIdentity};
use dog_device_client::session::{self, CommandHandler, SessionConfig};
use dog_device_client::{
    ClientError, CommandResult, DeviceRegistration, ReconnectPolicy, ServerCommand, TlsOptions,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
dog-device-client — robot dog device client

USAGE:
    dog-device-client [options]
    dog-device-client --print-device-id
    dog-device-client --explain-device-id

The device id is derived from hardware information (board serial / eMMC CID /
DMI UUID / physical MAC, falling back to /etc/machine-id) so every device gets
its own stable id. Use --device-id to override it.

The client keeps the WebSocket link alive and reconnects automatically with
exponential backoff (configurable, enabled by default).

OPTIONS:
    --server <URL>        Server base url            [env DOG_SERVER, default http://127.0.0.1:3000]
    --auth-secret <SECRET>
                          Shared secret used to sign the device token request.
                          Must match APP_DEVICE_AUTH_SECRET on the server.
                                                             [env DOG_AUTH_SECRET]
    --device-id <ID>      Override the hardware derived device id [env DOG_DEVICE_ID]
    --device-id-file <PATH>
                          Pin the detected id in this file and reuse it later
                          (recommended for systemd services, see README)
    --print-device-id     Print the detected device id and exit
    --explain-device-id   Print every probed hardware source and its status
    --device-type <TYPE>  Device type reported on register
    --ip <ADDR>           Device ip reported on register
    --interval <SECS>     Heartbeat interval                      [default 30]
    --heartbeat-timeout <SECS>
                          Heartbeat acknowledgement timeout       [default 10]
    --upload <PATH>       Upload this file once after the link is up
    --exec <COMMAND>      Shell command executed for every received task
                          (the task JSON is passed as DOG_COMMAND_JSON)
    --map-package <PATH>  Local ROS map package (.zip); its SHA-256 is compared
                          with the mapHash carried by every route task
    --map-hash <HEX>      Explicit local map SHA-256 (takes precedence over
                          --map-package)
    --ca <PEM>            CA certificate for wss:// (defaults to system roots)
    --cert <PEM>          Client certificate for mutual TLS
    --key <PEM>           Client private key for mutual TLS
    --no-reconnect        Exit when the link drops instead of retrying
    --reconnect-delay <SECS>      First retry delay               [default 1]
    --reconnect-max-delay <SECS>  Maximum retry delay             [default 30]
    --max-reconnect-attempts <N>  Give up after N failures        [default unlimited]
    --once                Authenticate, register, verify one heartbeat, exit
    -h, --help            Show this help
";

#[derive(Debug, Clone)]
struct Config {
    server: String,
    auth_secret: Option<String>,
    device_id: Option<String>,
    device_id_file: Option<PathBuf>,
    device_type: Option<String>,
    device_ip: Option<String>,
    interval_secs: u64,
    heartbeat_timeout_secs: u64,
    upload: Option<PathBuf>,
    exec: Option<String>,
    map_package: Option<PathBuf>,
    map_hash: Option<String>,
    ca: Option<PathBuf>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    once: bool,
    print_device_id: bool,
    explain_device_id: bool,
    no_reconnect: bool,
    reconnect_delay_secs: u64,
    reconnect_max_delay_secs: u64,
    max_reconnect_attempts: Option<u32>,
}

impl Config {
    fn tls(&self) -> Option<TlsOptions> {
        if self.ca.is_none() && self.cert.is_none() && self.key.is_none() {
            return None;
        }
        Some(TlsOptions {
            ca_pem: self.ca.clone(),
            client_cert_pem: self.cert.clone(),
            client_key_pem: self.key.clone(),
        })
    }

    fn registration(&self, identity: &DeviceIdentity) -> DeviceRegistration {
        DeviceRegistration {
            device_id: identity.id.clone(),
            device_ip: self.device_ip.clone(),
            device_type: self.device_type.clone(),
            model: None,
            os: std::env::consts::OS.to_string().into(),
            os_version: None,
            arch: std::env::consts::ARCH.to_string().into(),
            device_id_source: Some(identity.source.to_string()),
        }
    }

    fn reconnect_policy(&self) -> ReconnectPolicy {
        if self.once || self.no_reconnect {
            return ReconnectPolicy::disabled();
        }

        let interval = self.interval_secs.max(1);
        ReconnectPolicy::default()
            .with_initial_delay(Duration::from_secs(self.reconnect_delay_secs.max(1)))
            .with_max_delay(Duration::from_secs(self.reconnect_max_delay_secs.max(1)))
            .with_max_attempts(self.max_reconnect_attempts)
            .with_healthy_after(Duration::from_secs((interval * 3).clamp(10, 120)))
    }

    fn session(&self, identity: &DeviceIdentity) -> SessionConfig {
        let mut session = SessionConfig::new(&self.server, self.registration(identity));
        session.tls = self.tls();
        session.device_auth_secret = self.auth_secret.clone();
        session.heartbeat_interval = Duration::from_secs(self.interval_secs.max(1));
        session.heartbeat_timeout = Duration::from_secs(self.heartbeat_timeout_secs.max(1));
        session.upload = self.upload.clone();
        session.reconnect = self.reconnect_policy();
        session
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("dog_device_client=info")),
        )
        // keep stdout clean for `--print-device-id`
        .with_writer(std::io::stderr)
        .init();

    let config = match parse_args(std::env::args().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    // `--explain-device-id` reports the probe result itself (including the
    // "nothing usable" case), so it runs before resolving.
    if config.explain_device_id {
        print_device_id_report();
        return ExitCode::SUCCESS;
    }

    // Resolve the device id: explicit override, pinned file, hardware detection.
    let identity = match device_id::resolve_persisted(
        config.device_id.as_deref(),
        config.device_id_file.as_deref(),
    ) {
        Ok(identity) => identity,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    if config.print_device_id {
        println!("{}", identity.id);
        eprintln!("source: {}", identity.source);
        return ExitCode::SUCCESS;
    }

    info!(
        device_id = %identity.id,
        source = identity.source,
        reconnect = !config.once && !config.no_reconnect,
        "device id resolved"
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start tokio runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(config, identity)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(%error, "device client failed");
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config, identity: DeviceIdentity) -> Result<(), ClientError> {
    let session_config = config.session(&identity);

    if config.once {
        let device = session::verify_once(&session_config).await?;
        info!(
            device_id = %device.device_id,
            status = %device.status,
            "--once requested, exiting"
        );
        return Ok(());
    }

    let local_map_hash = match (&config.map_hash, &config.map_package) {
        (Some(hash), _) => Some(hash.trim().to_lowercase()),
        (None, Some(path)) => Some(dog_device_client::map::sha256_file(path)?),
        (None, None) => None,
    };
    if local_map_hash.is_none() {
        warn!(
            "no --map-package / --map-hash configured; route tasks carrying a map baseline will be rejected"
        );
    }

    let shutdown = spawn_shutdown_signal();
    let handler = ShellHandler {
        exec: config.exec.clone(),
        map_hash: local_map_hash,
    };

    let summary = session::run(session_config, handler, shutdown).await?;
    info!(
        connects = summary.connects,
        retries = summary.retries,
        "device client stopped"
    );
    Ok(())
}

/// Resolves once SIGINT (Ctrl-C) or SIGTERM (systemd stop) is received.
fn spawn_shutdown_signal() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    warn!(%error, "cannot listen for SIGTERM, using Ctrl-C only");
                    let _ = tokio::signal::ctrl_c().await;
                    let _ = tx.send(true);
                    return;
                }
            };

            tokio::select! {
                _ = tokio::signal::ctrl_c() => info!("SIGINT received, shutting down"),
                _ = terminate.recv() => info!("SIGTERM received, shutting down"),
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            info!("Ctrl-C received, shutting down");
        }

        let _ = tx.send(true);
    });

    rx
}

/// Executes pushed tasks with `--exec` (or simply acknowledges them).
struct ShellHandler {
    exec: Option<String>,
    /// SHA-256 of the locally installed ROS map package.
    map_hash: Option<String>,
}

impl ShellHandler {
    /// Reject a route task when the device cannot prove it holds the same map.
    fn verify_map_baseline(&self, command: &ServerCommand) -> Option<CommandResult> {
        let map = command.map_baseline()?;
        let command_id = command.id().unwrap_or_default();
        let Some(expected) = map
            .map_hash
            .as_deref()
            .map(str::trim)
            .filter(|hash| !hash.is_empty())
        else {
            return Some(CommandResult::failure(
                command_id,
                "task map baseline is missing mapHash",
            ));
        };
        let Some(local) = self.map_hash.as_deref() else {
            return Some(CommandResult::failure(
                command_id,
                "device has no local map hash; start with --map-package or --map-hash",
            ));
        };
        if !dog_device_client::map::hash_matches(expected, local) {
            return Some(CommandResult::failure(
                command_id,
                format!(
                    "map hash mismatch: expected {expected}, local {local} (mapId={:?}, version={:?})",
                    map.map_id, map.map_version
                ),
            ));
        }
        info!(
            map_id = ?map.map_id,
            map_version = ?map.map_version,
            frame_id = ?map.frame_id,
            "map baseline verified"
        );
        None
    }
}

impl CommandHandler for ShellHandler {
    async fn handle(&mut self, command: ServerCommand) -> CommandResult {
        let Some(command_id) = command.id() else {
            warn!(?command, "received a task without command id, ignored");
            return CommandResult::failure(0, "missing command id");
        };

        info!(
            command_id,
            name = ?command.command_name,
            kind = ?command.command_type,
            task_id = ?command.task_id,
            params = %command.params_text(),
            "task received"
        );

        if let Some(rejected) = self.verify_map_baseline(&command) {
            warn!(command_id, reason = ?rejected.return_value, "task rejected: map baseline check failed");
            return rejected;
        }

        let result = match &self.exec {
            Some(script) => {
                let params_text = command.params_text();
                match tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(script)
                    .env("DOG_COMMAND_ID", command_id.to_string())
                    .env("DOG_COMMAND_JSON", &params_text)
                    .output()
                    .await
                {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                        if output.status.success() {
                            CommandResult::success(command_id, stdout)
                        } else {
                            CommandResult::failure(
                                command_id,
                                format!(
                                    "exit={:?} stdout={} stderr={}",
                                    output.status.code(),
                                    stdout,
                                    stderr
                                ),
                            )
                        }
                    }
                    Err(error) => CommandResult::failure(
                        command_id,
                        format!("failed to execute task command: {error}"),
                    ),
                }
            }
            None => {
                CommandResult::success(command_id, "received").with_params(command.params.clone())
            }
        };

        info!(command_id, status = %result.status, "task result");
        result
    }
}

/// Print every probed hardware source so field engineers can see why a device
/// got a particular id (especially useful on Ubuntu, where some `sysfs` entries
/// are only readable by root).
fn print_device_id_report() {
    let report = device_id::explain();

    match &report.identity {
        Some(identity) => {
            println!("device id : {}", identity.id);
            println!("source    : {}", identity.source);
        }
        None => {
            println!("device id : <none> (pass --device-id or run as root)");
        }
    }
    println!("candidates:");

    let is_virtual_iface = |candidate: &device_id::Candidate| {
        candidate.source == device_id::SOURCE_MAC_ADDRESS
            && matches!(&candidate.status, CandidateStatus::Rejected(reason) if reason == "virtual interface")
    };
    let virtual_count = report
        .candidates
        .iter()
        .filter(|candidate| is_virtual_iface(candidate))
        .count();
    let mut virtual_printed = false;

    for candidate in &report.candidates {
        // A container host can have dozens of docker/bridge interfaces; the
        // report keeps them, the console output collapses them.
        if is_virtual_iface(candidate) {
            if !virtual_printed {
                println!("  SKIP     mac-address            ({virtual_count} virtual interfaces)");
                virtual_printed = true;
            }
            continue;
        }

        let (label, detail) = match &candidate.status {
            CandidateStatus::Found(id) => ("FOUND", id.clone()),
            CandidateStatus::Rejected(reason) => ("SKIP", reason.clone()),
            CandidateStatus::PermissionDenied => (
                "DENIED",
                "permission denied (run as root to use)".to_string(),
            ),
            CandidateStatus::Missing => ("MISSING", String::new()),
            CandidateStatus::Error(error) => ("ERROR", error.clone()),
        };
        println!(
            "  {label:<8} {:<22} {:<52} {detail}",
            candidate.source, candidate.location
        );
    }

    if report.has_permission_denied() {
        println!();
        println!(
            "hint: some hardware identifiers are root-only on Ubuntu; run as root or as a \
             systemd service to use a more stable id"
        );
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Config>, String> {
    let mut server = std::env::var("DOG_SERVER").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let mut auth_secret = std::env::var("DOG_AUTH_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut device_id_file = None;
    let mut device_id = std::env::var("DOG_DEVICE_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut device_type = None;
    let mut device_ip = None;
    let mut interval_secs = 30u64;
    let mut heartbeat_timeout_secs = 10u64;
    let mut upload = None;
    let mut exec = None;
    let mut map_package = None;
    let mut map_hash = None;
    let mut ca = None;
    let mut cert = None;
    let mut key = None;
    let mut once = false;
    let mut print_device_id = false;
    let mut explain_device_id = false;
    let mut no_reconnect = false;
    let mut reconnect_delay_secs = 1u64;
    let mut reconnect_max_delay_secs = 30u64;
    let mut max_reconnect_attempts = None;

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--server" => server = next_value(&mut args, "--server")?,
            "--auth-secret" => {
                auth_secret = Some(next_value(&mut args, "--auth-secret")?);
            }
            "--device-id" => device_id = Some(next_value(&mut args, "--device-id")?),
            "--device-id-file" => {
                device_id_file = Some(PathBuf::from(next_value(&mut args, "--device-id-file")?));
            }
            "--device-type" => device_type = Some(next_value(&mut args, "--device-type")?),
            "--ip" => device_ip = Some(next_value(&mut args, "--ip")?),
            "--interval" => {
                interval_secs = parse_u64(&mut args, "--interval")?;
            }
            "--heartbeat-timeout" => {
                heartbeat_timeout_secs = parse_u64(&mut args, "--heartbeat-timeout")?;
            }
            "--upload" => upload = Some(PathBuf::from(next_value(&mut args, "--upload")?)),
            "--exec" => exec = Some(next_value(&mut args, "--exec")?),
            "--map-package" => {
                map_package = Some(PathBuf::from(next_value(&mut args, "--map-package")?));
            }
            "--map-hash" => map_hash = Some(next_value(&mut args, "--map-hash")?),
            "--ca" => ca = Some(PathBuf::from(next_value(&mut args, "--ca")?)),
            "--cert" => cert = Some(PathBuf::from(next_value(&mut args, "--cert")?)),
            "--key" => key = Some(PathBuf::from(next_value(&mut args, "--key")?)),
            "--once" => once = true,
            "--print-device-id" => print_device_id = true,
            "--explain-device-id" => explain_device_id = true,
            "--no-reconnect" => no_reconnect = true,
            "--reconnect-delay" => {
                reconnect_delay_secs = parse_u64(&mut args, "--reconnect-delay")?;
            }
            "--reconnect-max-delay" => {
                reconnect_max_delay_secs = parse_u64(&mut args, "--reconnect-max-delay")?;
            }
            "--max-reconnect-attempts" => {
                max_reconnect_attempts =
                    Some(parse_u64(&mut args, "--max-reconnect-attempts")? as u32);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    Ok(Some(Config {
        server,
        auth_secret,
        device_id,
        device_id_file,
        device_type,
        device_ip,
        interval_secs,
        heartbeat_timeout_secs,
        upload,
        exec,
        map_package,
        map_hash,
        ca,
        cert,
        key,
        once,
        print_device_id,
        explain_device_id,
        no_reconnect,
        reconnect_delay_secs,
        reconnect_max_delay_secs,
        max_reconnect_attempts,
    }))
}

fn next_value(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    name: &str,
) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{name} expects a value"))
}

fn parse_u64(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    name: &str,
) -> Result<u64, String> {
    let value = next_value(args, name)?;
    value
        .parse()
        .map_err(|_| format!("{name} expects a number, got `{value}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command_with_map(map_hash: Option<&str>) -> ServerCommand {
        let mut map = serde_json::Map::new();
        map.insert("mapId".to_string(), json!(3));
        map.insert("mapVersion".to_string(), json!("v1.0"));
        map.insert("frameId".to_string(), json!("map"));
        if let Some(hash) = map_hash {
            map.insert("mapHash".to_string(), json!(hash));
        }
        serde_json::from_value(json!({
            "type": "server-command",
            "commandId": 7,
            "params": { "mission": "patrol", "map": map }
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn accepts_task_when_local_map_hash_matches() {
        let mut handler = ShellHandler {
            exec: None,
            map_hash: Some("abc123".to_string()),
        };
        let result = handler.handle(command_with_map(Some("ABC123"))).await;
        assert_eq!(result.status, "success");
    }

    #[tokio::test]
    async fn rejects_task_when_local_map_hash_differs() {
        let mut handler = ShellHandler {
            exec: None,
            map_hash: Some("abc123".to_string()),
        };
        let result = handler.handle(command_with_map(Some("deadbeef"))).await;
        assert_eq!(result.status, "error");
        assert!(result.return_value.unwrap().contains("map hash mismatch"));
    }

    #[tokio::test]
    async fn rejects_map_task_without_local_hash() {
        let mut handler = ShellHandler {
            exec: None,
            map_hash: None,
        };
        let result = handler.handle(command_with_map(Some("abc123"))).await;
        assert_eq!(result.status, "error");
        assert!(result.return_value.unwrap().contains("no local map hash"));
    }

    #[tokio::test]
    async fn rejects_map_task_without_server_hash() {
        let mut handler = ShellHandler {
            exec: None,
            map_hash: Some("abc123".to_string()),
        };
        let result = handler.handle(command_with_map(None)).await;
        assert_eq!(result.status, "error");
        assert!(result.return_value.unwrap().contains("missing mapHash"));
    }

    #[tokio::test]
    async fn tasks_without_map_payload_still_run() {
        let mut handler = ShellHandler {
            exec: None,
            map_hash: None,
        };
        let command: ServerCommand = serde_json::from_value(json!({
            "type": "server-command",
            "commandId": 8,
            "params": { "mission": "patrol" }
        }))
        .unwrap();
        let result = handler.handle(command).await;
        assert_eq!(result.status, "success");
    }
}
