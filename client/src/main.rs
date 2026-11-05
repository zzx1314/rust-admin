use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use dog_device_client::{
    ClientError, CommandResult, DeviceClient, DeviceRegistration, Heartbeat, ServerCommand,
    Telemetry, TlsOptions,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
dog-device-client — robot dog device client

USAGE:
    dog-device-client --device-id <ID> [options]

OPTIONS:
    --server <URL>        Server base url            [env DOG_SERVER, default http://127.0.0.1:3000]
    --device-id <ID>      Device id (required)       [env DOG_DEVICE_ID]
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
    device_id: String,
    device_type: Option<String>,
    device_ip: Option<String>,
    interval_secs: u64,
    upload: Option<PathBuf>,
    exec: Option<String>,
    ca: Option<PathBuf>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    once: bool,
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

    fn registration(&self) -> DeviceRegistration {
        DeviceRegistration {
            device_id: self.device_id.clone(),
            device_ip: self.device_ip.clone(),
            device_type: self.device_type.clone(),
            model: None,
            os: std::env::consts::OS.to_string().into(),
            os_version: None,
            arch: std::env::consts::ARCH.to_string().into(),
        }
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("dog_device_client=info")),
        )
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

    match runtime.block_on(run(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(%error, "device client failed");
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> Result<(), ClientError> {
    let tls = config.tls();

    let mut client = DeviceClient::new(&config.server, &config.device_id);
    if let Some(tls) = &tls {
        client = client.with_tls(tls)?;
    }

    // 1. device authentication (access token)
    let token = client.authenticate().await?;
    info!(
        device_id = %config.device_id,
        token = %mask(&token),
        "device token acquired"
    );

    // 2. device registration (creates/refreshes the device record)
    let device = client.register(&config.registration()).await?;
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

    let heartbeat = Heartbeat::online(config.device_id.clone()).with_telemetry(Telemetry {
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
    let mut device_id = std::env::var("DOG_DEVICE_ID").unwrap_or_default();
    let mut device_type = None;
    let mut device_ip = None;
    let mut interval_secs = 30u64;
    let mut upload = None;
    let mut exec = None;
    let mut ca = None;
    let mut cert = None;
    let mut key = None;
    let mut once = false;

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--server" => server = next_value(&mut args, "--server")?,
            "--device-id" => device_id = next_value(&mut args, "--device-id")?,
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
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    if device_id.trim().is_empty() {
        return Err("--device-id is required".to_string());
    }

    Ok(Some(Config {
        server,
        device_id,
        device_type,
        device_ip,
        interval_secs,
        upload,
        exec,
        ca,
        cert,
        key,
        once,
    }))
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
