//! Dog device client: authenticate, register, keep a WebSocket task link with
//! the server, report results and upload files.
//!
//! ```no_run
//! # async fn example() -> Result<(), dog_device_client::ClientError> {
//! use dog_device_client::{DeviceClient, DeviceRegistration, Heartbeat};
//!
//! let mut client = DeviceClient::new("http://127.0.0.1:3000", "dog-001");
//! client.authenticate().await?;
//! client
//!     .register(&DeviceRegistration {
//!         device_id: "dog-001".to_string(),
//!         ..Default::default()
//!     })
//!     .await?;
//!
//! let mut link = client.connect().await?;
//! let ack = link
//!     .heartbeat_with_ack(&Heartbeat::online("dog-001"), std::time::Duration::from_secs(5))
//!     .await?;
//! assert_eq!(ack["status"], "online");
//! # Ok(())
//! # }
//! ```

pub mod auth;
pub mod device_auth;
pub mod device_id;
pub mod error;
pub mod link;
pub mod map;
pub mod protocol;
pub mod reconnect;
pub mod session;
pub mod tls;

pub use auth::DeviceClient;
pub use device_auth::DeviceSignature;
pub use device_id::DeviceIdentity;
pub use error::ClientError;
pub use link::DeviceLink;
pub use map::{hash_matches, sha256_bytes, sha256_file};
pub use protocol::{
    CommandResult, DeviceRegistration, DeviceSummary, FileAck, Heartbeat, MapBaseline,
    ServerCommand, Telemetry,
};
pub use reconnect::ReconnectPolicy;
pub use session::{CommandHandler, SessionConfig, SessionSummary};
pub use tls::TlsOptions;
