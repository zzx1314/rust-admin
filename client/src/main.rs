use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use dog_device_client::device_id::{self, CandidateStatus, DeviceIdentity};
use dog_device_client::{
    ClientError, CommandResult, DeviceClient, DeviceRegistration, Heartbeat, ServerCommand,
    Telemetry, TlsOptions,
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

OPTIONS:
    --server <URL>        Server base url            [env DOG_SERVER, default http://127.0.0.1:3000]
    --device-id <ID>      Override the hardware derived device id [env DOG_DEVICE_ID]
    --device-id-file <PATH>
                          Pin the detected id in this file and reuse it later
                          (recommended for systemd services, see README)
    --print-device-id     Print the detected device id and exit
    --explain-device-id   Print every probed hardware source and its status
    --device-type <TYPE>  Device type reported on register
    --ip <ADDR>           Device ip reported on register
    --interval <SECS>     Heartbeat interval         [default 30]
    --upload <PATH>       Upload this file once after the link is up
    --exec <COMMAND>      Shell command executed for every received task
                          (the task JSON is passed as DOG_COMMAND_JSON)
    --ca <PEM>            CA certificate for wss:// (defaults to system roots)
    --cert <PEM>          Client certificate for mutual TLS
    --key <PEM>           Client private key for mutual TLS
    --once                Authenticate, register, verify one heartbeat, exit
    -h, --help            Show this help
";

#[derive(Debug, Clone)]
struct Config {
    server: String,
    device_id: Option<String>,
    device_id_file: Option<PathBuf>,
    device_type: Option<String>,
    device_ip: Option<String>,
    interval_secs: u64,
    upload: Option<PathBuf>,
    exec: Option<String>,
    ca: Option<PathBuf>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    once: bool,
    print_device_id: bool,
    explain_device_id: bool,
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

    // Resolve the device id: explicit override, otherwise hardware detection.
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
    let tls = config.tls();

    let mut client = DeviceClient::new(&config.server, &identity.id);
    if let Some(tls) = &tls {
        client = client.with_tls(tls)?;
    }

    // 1. device authentication (access token)
    let token = client.authenticate().await?;
    info!(
        device_id = %identity.id,
        device_id_source = identity.source,
        token = %mask(&token),
        "device token acquired"
    );

    // 2. device registration (creates/refreshes the device record)
    let device = client.register(&config.registration(&identity)).await?;
    info!(
        device_id = %device.device_id,
        status = %device.status,
        device_type = ?device.device_type,
        "device registered"
    );

    // 3. websocket link + heartbeat verification
    info!(url = %client.websocket_url()?, "opening device websocket");
    let mut link = match &tls {
        Some(tls) => client.connect_with_tls(tls).await?,
        None => client.connect().await?,
    };

    let heartbeat = Heartbeat::online(identity.id.clone()).with_telemetry(Telemetry {
        battery: Some("100".to_string()),
        ..Telemetry::default()
    });

    let ack = link
        .heartbeat_with_ack(&heartbeat, Duration::from_secs(10))
        .await?;
    info!(ack = %ack, "heartbeat acknowledged: link verified");

    if config.once {
        link.close().await?;
        info!("--once requested, exiting");
        return Ok(());
    }

    // 4. optional one-shot file upload
    if let Some(path) = &config.upload {
        let ack = link
            .upload_file(path, Some("device_upload"), None, Duration::from_secs(120))
            .await?;
        info!(
            file = %path.display(),
            status = %ack.status,
            stored = ?ack.path,
            "file upload finished"
        );
    }

    // 5. main loop: heartbeat on idle + execute pushed tasks
    let interval = Duration::from_secs(config.interval_secs.max(1));
    let mut last_heartbeat = Instant::now();
    let mut shutdown = std::pin::pin!(tokio::signal::ctrl_c());
    let mut stop = false;

    loop {
        let command = tokio::select! {
            result = link.next_command(interval) => result?,
            _ = &mut shutdown => {
                stop = true;
                None
            }
        };

        if stop {
            break;
        }

        if let Some(command) = command {
            handle_command(&mut link, &config, command).await?;
        }

        if last_heartbeat.elapsed() >= interval {
            link.heartbeat(&heartbeat).await?;
            last_heartbeat = Instant::now();
            tracing::debug!("heartbeat sent");
        }
    }

    link.close().await?;
    info!("device client stopped");
    Ok(())
}

async fn handle_command(
    link: &mut dog_device_client::DeviceLink,
    config: &Config,
    command: ServerCommand,
) -> Result<(), ClientError> {
    let Some(command_id) = command.id() else {
        warn!(?command, "received a task without command id, ignored");
        return Ok(());
    };

    info!(
        command_id,
        name = ?command.command_name,
        kind = ?command.command_type,
        task_id = ?command.task_id,
        params = %command.params_text(),
        "task received"
    );

    let params_text = command.params_text();
    let result = match &config.exec {
        Some(exec) => {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(exec)
                .env("DOG_COMMAND_ID", command_id.to_string())
                .env("DOG_COMMAND_JSON", &params_text)
                .output()
                .await?;

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
        None => CommandResult::success(command_id, "received").with_params(command.params.clone()),
    };

    link.report(&result).await?;
    info!(command_id, status = %result.status, "task result reported");
    Ok(())
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Config>, String> {
    let mut server = std::env::var("DOG_SERVER").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let mut device_id_file = None;
    let mut device_id = std::env::var("DOG_DEVICE_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut device_type = None;
    let mut device_ip = None;
    let mut interval_secs = 30u64;
    let mut upload = None;
    let mut exec = None;
    let mut ca = None;
    let mut cert = None;
    let mut key = None;
    let mut once = false;
    let mut print_device_id = false;
    let mut explain_device_id = false;

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--server" => server = next_value(&mut args, "--server")?,
            "--device-id" => device_id = Some(next_value(&mut args, "--device-id")?),
            "--device-id-file" => {
                device_id_file = Some(PathBuf::from(next_value(&mut args, "--device-id-file")?));
            }
            "--device-type" => device_type = Some(next_value(&mut args, "--device-type")?),
            "--ip" => device_ip = Some(next_value(&mut args, "--ip")?),
            "--interval" => {
                interval_secs = next_value(&mut args, "--interval")?
                    .parse()
                    .map_err(|_| "--interval expects a number of seconds".to_string())?;
            }
            "--upload" => upload = Some(PathBuf::from(next_value(&mut args, "--upload")?)),
            "--exec" => exec = Some(next_value(&mut args, "--exec")?),
            "--ca" => ca = Some(PathBuf::from(next_value(&mut args, "--ca")?)),
            "--cert" => cert = Some(PathBuf::from(next_value(&mut args, "--cert")?)),
            "--key" => key = Some(PathBuf::from(next_value(&mut args, "--key")?)),
            "--once" => once = true,
            "--print-device-id" => print_device_id = true,
            "--explain-device-id" => explain_device_id = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    Ok(Some(Config {
        server,
        device_id,
        device_id_file,
        device_type,
        device_ip,
        interval_secs,
        upload,
        exec,
        ca,
        cert,
        key,
        once,
        print_device_id,
        explain_device_id,
    }))
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

fn next_value(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    name: &str,
) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{name} expects a value"))
}

fn mask(token: &str) -> String {
    if token.len() <= 8 {
        "*".repeat(token.len())
    } else {
        format!("{}***{}", &token[..4], &token[token.len() - 4..])
    }
}
