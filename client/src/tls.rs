use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};
use tokio_tungstenite::Connector;

use crate::error::ClientError;

/// TLS material used for mutual-TLS deployments (`wss://` plus client
/// certificate, like the original C client which used
/// `client.crt` / `client.key` / `rootca.crt`).
#[derive(Debug, Clone, Default)]
pub struct TlsOptions {
    /// PEM file with the CA that signed the server certificate.
    pub ca_pem: Option<PathBuf>,
    /// PEM file with the client certificate.
    pub client_cert_pem: Option<PathBuf>,
    /// PEM file with the client private key.
    pub client_key_pem: Option<PathBuf>,
}

impl TlsOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust a custom CA (instead of the system trust store).
    pub fn with_ca(mut self, path: impl Into<PathBuf>) -> Self {
        self.ca_pem = Some(path.into());
        self
    }

    /// Present a client certificate (mutual TLS).
    pub fn with_client_cert(
        mut self,
        cert_pem: impl Into<PathBuf>,
        key_pem: impl Into<PathBuf>,
    ) -> Self {
        self.client_cert_pem = Some(cert_pem.into());
        self.client_key_pem = Some(key_pem.into());
        self
    }

    /// Build the `rustls` connector used by `tokio-tungstenite`.
    pub fn connector(&self) -> Result<Connector, ClientError> {
        let roots = self.root_store()?;
        let builder =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|error| ClientError::Tls(error.to_string()))?
                .with_root_certificates(roots);

        let config = match (&self.client_cert_pem, &self.client_key_pem) {
            (Some(cert), Some(key)) => {
                let certs = read_certs(cert)?;
                let key = read_private_key(key)?;
                builder
                    .with_client_auth_cert(certs, key)
                    .map_err(|error| ClientError::Tls(error.to_string()))?
            }
            (None, None) => builder.with_no_client_auth(),
            _ => {
                return Err(ClientError::Tls(
                    "client certificate and key must be provided together".to_string(),
                ));
            }
        };

        Ok(Connector::Rustls(Arc::new(config)))
    }

    /// Build a `reqwest` client that uses the same trust material so the REST
    /// calls (auth/register) work against a mutual-TLS endpoint.
    pub fn http_client(&self) -> Result<reqwest::Client, ClientError> {
        let mut builder = reqwest::Client::builder();

        if let Some(ca) = &self.ca_pem {
            let pem = std::fs::read(ca)?;
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&pem)?);
        }

        if let (Some(cert), Some(key)) = (&self.client_cert_pem, &self.client_key_pem) {
            let mut pem = std::fs::read(cert)?;
            pem.extend_from_slice(&std::fs::read(key)?);
            builder = builder.identity(reqwest::Identity::from_pem(&pem)?);
        }

        Ok(builder.build()?)
    }

    fn root_store(&self) -> Result<RootCertStore, ClientError> {
        let mut roots = RootCertStore::empty();
        match &self.ca_pem {
            Some(path) => {
                for cert in read_certs(path)? {
                    roots
                        .add(cert)
                        .map_err(|error| ClientError::Tls(error.to_string()))?;
                }
            }
            None => {
                let native = rustls_native_certs::load_native_certs();
                for cert in native.certs {
                    let _ = roots.add(cert);
                }
            }
        }

        if roots.is_empty() {
            return Err(ClientError::Tls(
                "no trusted certificates available, provide a CA PEM file".to_string(),
            ));
        }

        Ok(roots)
    }
}

fn read_certs(path: &std::path::Path) -> Result<Vec<CertificateDer<'static>>, ClientError> {
    let mut reader = BufReader::new(File::open(path)?);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ClientError::Tls(format!("failed to read {}: {error}", path.display())))?;
    if certs.is_empty() {
        return Err(ClientError::Tls(format!(
            "{} does not contain any certificate",
            path.display()
        )));
    }
    Ok(certs)
}

fn read_private_key(path: &std::path::Path) -> Result<PrivateKeyDer<'static>, ClientError> {
    let mut reader = BufReader::new(File::open(path)?);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|error| ClientError::Tls(format!("failed to read {}: {error}", path.display())))?
        .ok_or_else(|| {
            ClientError::Tls(format!("{} does not contain a private key", path.display()))
        })
}
