use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::auth::DeviceClient;
use crate::error::ClientError;
use crate::link::DeviceLink;
use crate::protocol::{
    CommandResult, DeviceRegistration, DeviceSummary, Heartbeat, ServerCommand, Telemetry,
};
use crate::reconnect::ReconnectPolicy;
use crate::tls::TlsOptions;

/// Everything needed to keep a device online.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Server root url, e.g. `http://127.0.0.1:3000`.
    pub server: String,
    /// Registration payload (carries `device_id` and `device_id_source`).
    pub registration: DeviceRegistration,
    pub tls: Option<TlsOptions>,
    /// How often an idle link sends a heartbeat.
    pub heartbeat_interval: Duration,
    /// How long a heartbeat acknowledgement may take before the link is
    /// considered dead (this is what detects half-open TCP connections).
    pub heartbeat_timeout: Duration,
    /// Telemetry included in every heartbeat.
    pub telemetry: Telemetry,
    /// File uploaded once, on the first successful connection.
    pub upload: Option<PathBuf>,
    pub reconnect: ReconnectPolicy,
}

impl SessionConfig {
    pub fn new(server: impl Into<String>, registration: DeviceRegistration) -> Self {
        Self {
            server: server.into(),
            registration,
            tls: None,
            heartbeat_interval: Duration::from_secs(30),
            heartbeat_timeout: Duration::from_secs(10),
            telemetry: Telemetry::default(),
            upload: None,
            reconnect: ReconnectPolicy::default(),
        }
    }

    pub fn device_id(&self) -> &str {
        &self.registration.device_id
    }
}

/// Counters reported when a session supervisor stops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionSummary {
    /// Successful connections (first connect + reconnects).
    pub connects: u32,
    /// Failed attempts that triggered a retry.
    pub retries: u32,
}

/// Handles a task pushed by the server.
///
/// Implementations must be `Send` because the supervisor is usually driven from
/// a background task.
pub trait CommandHandler: Send {
    fn handle(
        &mut self,
        command: ServerCommand,
    ) -> impl std::future::Future<Output = CommandResult> + Send;
}

/// Keep the device connected until `shutdown` is signalled.
///
/// Any transport problem (server restart, Wi-Fi drop, half-open TCP, expired
/// token) is retried with exponential backoff; only configuration errors are
/// fatal. Passing a policy with `max_attempts = Some(0)` disables retries.
pub async fn run<H: CommandHandler>(
    config: SessionConfig,
    mut handler: H,
    mut shutdown: watch::Receiver<bool>,
) -> Result<SessionSummary, ClientError> {
    let mut summary = SessionSummary::default();
    let mut failures: u32 = 0;
    let mut upload = config.upload.clone();

    loop {
        if *shutdown.borrow() {
            return Ok(summary);
        }

        let started = Instant::now();
        let mut connected = false;
        let outcome = run_once(
            &config,
            &mut handler,
            &mut shutdown,
            &mut upload,
            &mut connected,
        )
        .await;

        if connected {
            summary.connects += 1;
        }

        match outcome {
            // graceful shutdown requested
            Ok(()) => return Ok(summary),
            Err(error) if is_fatal(&error) => return Err(error),
            Err(error) => {
                summary.retries += 1;
                let uptime = started.elapsed();
                if uptime >= config.reconnect.healthy_after {
                    failures = 0;
                }
                warn!(
                    %error,
                    uptime_secs = uptime.as_secs(),
                    "device link lost"
                );
            }
        }

        if !config.reconnect.allows_attempt(failures) {
            return Err(ClientError::Protocol(format!(
                "giving up after {failures} reconnect attempt(s)"
            )));
        }

        failures += 1;
        let delay = config.reconnect.delay(failures);
        info!(
            attempt = failures,
            delay_ms = delay.as_millis() as u64,
            "reconnecting to the server"
        );

        if !sleep_or_shutdown(delay, &mut shutdown).await {
            return Ok(summary);
        }
    }
}

/// Authenticate, register, connect and verify a single heartbeat, then close.
///
/// Used by `--once` as a deployment smoke test (no retries).
pub async fn verify_once(config: &SessionConfig) -> Result<DeviceSummary, ClientError> {
    let mut client = build_client(&config.server, config.device_id(), config.tls.as_ref())?;
    client.authenticate().await?;
    let device = client.register(&config.registration).await?;

    let mut link = connect(&client, config).await?;
    let heartbeat = heartbeat_instance(config);
    let ack = link
        .heartbeat_with_ack(&heartbeat, config.heartbeat_timeout)
        .await?;
    info!(ack = %ack, "link verified");
    link.close().await?;

    Ok(device)
}

/// One connection lifecycle: returns `Ok(())` only when shutdown was requested.
async fn run_once(
    config: &SessionConfig,
    handler: &mut impl CommandHandler,
    shutdown: &mut watch::Receiver<bool>,
    upload: &mut Option<PathBuf>,
    connected: &mut bool,
) -> Result<(), ClientError> {
    let mut client = build_client(&config.server, config.device_id(), config.tls.as_ref())?;

    // Re-authenticate on every (re)connect: tokens are short lived and the
    // handshake refreshes them, so a reconnect after a long outage just works.
    let token = client.authenticate().await?;
    let device = client.register(&config.registration).await?;
    info!(
        device_id = %device.device_id,
        status = %device.status,
        token = %mask_token(&token),
        "device registered"
    );

    let mut link = connect(&client, config).await?;
    *connected = true;
    let heartbeat = heartbeat_instance(config);
    let ack = link
        .heartbeat_with_ack(&heartbeat, config.heartbeat_timeout)
        .await?;
    *connected = true;
    info!(ack = %ack, "device link established");

    if let Some(path) = upload.take() {
        match link
            .upload_file(&path, Some("device_upload"), None, Duration::from_secs(120))
            .await
        {
            Ok(ack) => info!(
                file = %path.display(),
                status = %ack.status,
                stored = ?ack.path,
                "file upload finished"
            ),
            Err(error) => {
                // keep it pending so the next connection retries the upload
                warn!(file = %path.display(), %error, "file upload failed, will retry");
                *upload = Some(path);
            }
        }
    }

    let mut last_heartbeat = Instant::now();

    loop {
        let command = tokio::select! {
            result = link.next_command(config.heartbeat_interval) => result?,
            _ = shutdown.changed() => {
                if let Err(error) = link.close().await {
                    debug!(%error, "failed to close link gracefully");
                }
                return Ok(());
            }
        };

        if let Some(command) = command {
            let result = handler.handle(command).await;
            link.report(&result).await?;
        }

        if last_heartbeat.elapsed() >= config.heartbeat_interval {
            let ack = link
                .heartbeat_with_ack(&heartbeat, config.heartbeat_timeout)
                .await?;
            debug!(ack = %ack, "heartbeat acknowledged");
            last_heartbeat = Instant::now();
        }
    }
}

fn build_client(
    server: &str,
    device_id: &str,
    tls: Option<&TlsOptions>,
) -> Result<DeviceClient, ClientError> {
    let client = DeviceClient::new(server, device_id);
    match tls {
        Some(tls) => client.with_tls(tls),
        None => Ok(client),
    }
}

async fn connect(client: &DeviceClient, config: &SessionConfig) -> Result<DeviceLink, ClientError> {
    match &config.tls {
        Some(tls) => client.connect_with_tls(tls).await,
        None => client.connect().await,
    }
}

fn heartbeat_instance(config: &SessionConfig) -> Heartbeat {
    Heartbeat::online(config.registration.device_id.clone())
        .with_telemetry(config.telemetry.clone())
}

/// Configuration errors are not retryable; transport problems are.
fn is_fatal(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::DeviceId(_)
            | ClientError::Tls(_)
            | ClientError::Url(_)
            | ClientError::MissingToken
    )
}

/// Sleep for `delay`, waking early when shutdown is signalled.
/// Returns `false` when the caller should stop.
async fn sleep_or_shutdown(delay: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    if delay.is_zero() {
        return !*shutdown.borrow();
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => !*shutdown.borrow(),
        _ = shutdown.changed() => false,
    }
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        "*".repeat(token.len())
    } else {
        format!("{}***{}", &token[..4], &token[token.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatal_errors_are_not_retried() {
        assert!(is_fatal(&ClientError::Tls("bad cert".to_string())));
        assert!(is_fatal(&ClientError::DeviceId("none".to_string())));
        assert!(is_fatal(&ClientError::Url("bad url".to_string())));
        assert!(is_fatal(&ClientError::MissingToken));
    }

    #[test]
    fn transport_errors_are_retried() {
        assert!(!is_fatal(&ClientError::Timeout(
            "heartbeat acknowledgement"
        )));
        assert!(!is_fatal(&ClientError::Protocol("closed".to_string())));
        assert!(!is_fatal(&ClientError::Server {
            code: 10400,
            msg: "bad token".to_string(),
        }));
    }

    #[tokio::test]
    async fn shutdown_before_connect_returns_immediately() {
        struct NoopHandler;
        impl CommandHandler for NoopHandler {
            async fn handle(&mut self, command: ServerCommand) -> CommandResult {
                CommandResult::success(command.id().unwrap_or_default(), "noop")
            }
        }

        let (tx, rx) = watch::channel(true);
        drop(tx);

        let config = SessionConfig::new(
            "http://127.0.0.1:1",
            DeviceRegistration {
                device_id: "test-device".to_string(),
                ..DeviceRegistration::default()
            },
        );
        let summary = run(config, NoopHandler, rx).await.unwrap();
        assert_eq!(summary, SessionSummary::default());
    }

    #[tokio::test]
    async fn retries_until_the_attempt_limit_when_the_server_is_unreachable() {
        struct NoopHandler;
        impl CommandHandler for NoopHandler {
            async fn handle(&mut self, command: ServerCommand) -> CommandResult {
                CommandResult::success(command.id().unwrap_or_default(), "noop")
            }
        }

        let (_tx, rx) = watch::channel(false);
        let mut config = SessionConfig::new(
            // discard port: connection is refused immediately
            "http://127.0.0.1:9",
            DeviceRegistration {
                device_id: "test-device".to_string(),
                ..DeviceRegistration::default()
            },
        );
        config.heartbeat_interval = Duration::from_millis(50);
        config.heartbeat_timeout = Duration::from_millis(50);
        config.reconnect = ReconnectPolicy::default()
            .with_initial_delay(Duration::from_millis(10))
            .with_max_delay(Duration::from_millis(20))
            .with_max_attempts(Some(2));

        let error = run(config, NoopHandler, rx).await.unwrap_err();
        match error {
            ClientError::Protocol(message) => {
                assert!(
                    message.contains("giving up"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected a giving-up protocol error, got {other:?}"),
        }
    }

    #[test]
    fn mask_token_hides_the_middle() {
        assert_eq!(mask_token("abcdefghijkl"), "abcd***ijkl");
        assert_eq!(mask_token("abc"), "***");
    }
}
