use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;

use kani_proto::codec::{self, MAX_EVENT_SIZE};
use kani_proto::event::InputEvent;
use ring::digest;
use webrtc_dtls::config::{ClientAuthType, Config, ExtendedMasterSecretType};
use webrtc_dtls::conn::DTLSConn;
use webrtc_dtls::crypto::{Certificate, CryptoPrivateKey};
use webrtc_util::conn::{Conn, Listener};

#[derive(Debug, Error)]
pub enum DtlsError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("DTLS error: {0}")]
    Dtls(#[from] webrtc_dtls::Error),
    #[error("codec error: {0}")]
    Codec(#[from] codec::CodecError),
    #[error("certificate error: {0}")]
    Certificate(String),
    #[error("fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch { expected: String, actual: String },
}

/// A DTLS-encrypted transport connection.
pub struct DtlsTransport {
    conn: Arc<dyn Conn + Send + Sync>,
    peer_fingerprint: String,
    own_fingerprint: String,
}

/// Generate a self-signed certificate and return (Certificate, DER bytes for fingerprinting).
pub fn generate_self_signed_cert() -> Result<(Certificate, Vec<u8>), DtlsError> {
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|e| DtlsError::Certificate(e.to_string()))?;
    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .map_err(|e| DtlsError::Certificate(e.to_string()))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| DtlsError::Certificate(e.to_string()))?;
    let cert_der = cert.der().to_vec();
    let private_key = CryptoPrivateKey::from_key_pair(&key_pair)
        .map_err(|e| DtlsError::Certificate(e.to_string()))?;
    let certificate = Certificate {
        certificate: vec![cert.into()],
        private_key,
    };
    Ok((certificate, cert_der))
}

/// Compute SHA-256 fingerprint of DER-encoded certificate.
pub fn cert_fingerprint(der_bytes: &[u8]) -> String {
    let hash = digest::digest(&digest::SHA256, der_bytes);
    let hex: Vec<String> = hash.as_ref().iter().map(|b| format!("{b:02X}")).collect();
    format!("sha256:{}", hex.join(":"))
}

impl DtlsTransport {
    /// Connect as DTLS client to a server.
    /// If `expected_fingerprint` is Some, verify the server's cert matches (TOFU pinning).
    pub async fn connect(
        server_addr: SocketAddr,
        cert: Certificate,
        cert_der: &[u8],
        expected_fingerprint: Option<&str>,
    ) -> Result<Self, DtlsError> {
        let own_fp = cert_fingerprint(cert_der);

        let config = Config {
            certificates: vec![cert],
            insecure_skip_verify: true,
            extended_master_secret: ExtendedMasterSecretType::Require,
            server_name: "localhost".to_string(),
            ..Default::default()
        };

        let sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        sock.connect(server_addr).await?;
        let sock: Arc<dyn Conn + Send + Sync> = Arc::new(sock);

        let dtls_conn = DTLSConn::new(sock, config, true, None).await?;

        // Extract peer fingerprint
        let state = dtls_conn.connection_state().await;
        let peer_fp = state
            .peer_certificates
            .first()
            .map(|c| cert_fingerprint(c))
            .unwrap_or_default();

        // TOFU verification
        if let Some(expected) = expected_fingerprint {
            if peer_fp != expected {
                return Err(DtlsError::FingerprintMismatch {
                    expected: expected.to_string(),
                    actual: peer_fp,
                });
            }
        }

        let conn: Arc<dyn Conn + Send + Sync> = Arc::new(dtls_conn);
        Ok(Self {
            conn,
            peer_fingerprint: peer_fp,
            own_fingerprint: own_fp,
        })
    }

    /// Accept a DTLS connection as server.
    /// If `expected_fingerprint` is Some, verify the client's cert matches.
    pub async fn accept(
        bind_addr: String,
        cert: Certificate,
        cert_der: &[u8],
        expected_fingerprint: Option<&str>,
    ) -> Result<Self, DtlsError> {
        let own_fp = cert_fingerprint(cert_der);

        let config = Config {
            certificates: vec![cert],
            insecure_skip_verify: true,
            client_auth: ClientAuthType::RequireAnyClientCert,
            extended_master_secret: ExtendedMasterSecretType::Require,
            ..Default::default()
        };

        let listener = webrtc_dtls::listener::listen(bind_addr, config).await?;
        let (dtls_conn, _peer_addr): (Arc<dyn Conn + Send + Sync>, SocketAddr) = listener
            .accept()
            .await
            .map_err(|e| DtlsError::Io(std::io::Error::other(e.to_string())))?;

        // Extract peer fingerprint
        let peer_fp = if let Some(dtls) = dtls_conn.as_any().downcast_ref::<DTLSConn>() {
            let state: webrtc_dtls::state::State = dtls.connection_state().await;
            state
                .peer_certificates
                .first()
                .map(|c| cert_fingerprint(c))
                .unwrap_or_default()
        } else {
            String::new()
        };

        // TOFU verification
        if let Some(expected) = expected_fingerprint {
            if peer_fp != expected {
                return Err(DtlsError::FingerprintMismatch {
                    expected: expected.to_string(),
                    actual: peer_fp,
                });
            }
        }

        Ok(Self {
            conn: dtls_conn,
            peer_fingerprint: peer_fp,
            own_fingerprint: own_fp,
        })
    }

    pub fn peer_fingerprint(&self) -> &str {
        &self.peer_fingerprint
    }

    pub fn own_fingerprint(&self) -> &str {
        &self.own_fingerprint
    }

    pub async fn send_event(&self, event: &InputEvent) -> Result<(), DtlsError> {
        let bytes = codec::encode(event)?;
        self.conn
            .send(&bytes)
            .await
            .map_err(|e| DtlsError::Io(std::io::Error::other(e.to_string())))?;
        Ok(())
    }

    pub async fn recv_event(&self) -> Result<InputEvent, DtlsError> {
        let mut buf = vec![0u8; MAX_EVENT_SIZE];
        let n = self
            .conn
            .recv(&mut buf)
            .await
            .map_err(|e| DtlsError::Io(std::io::Error::other(e.to_string())))?;
        let event = codec::decode(&buf[..n])?;
        Ok(event)
    }

    pub async fn close(&self) -> Result<(), DtlsError> {
        self.conn
            .close()
            .await
            .map_err(|e| DtlsError::Io(std::io::Error::other(e.to_string())))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cert_fingerprint_format() {
        let (_, der) = generate_self_signed_cert().unwrap();
        let fp = cert_fingerprint(&der);
        assert!(fp.starts_with("sha256:"));
        // SHA-256 = 32 bytes = 32 hex pairs with colons = 32*3-1 = 95 chars + "sha256:" = 102
        assert_eq!(fp.len(), 7 + 95); // "sha256:" + "XX:XX:...:XX"
    }

    #[test]
    fn test_generate_cert_succeeds() {
        let result = generate_self_signed_cert();
        assert!(result.is_ok());
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let (_, der) = generate_self_signed_cert().unwrap();
        let fp1 = cert_fingerprint(&der);
        let fp2 = cert_fingerprint(&der);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_different_certs_different_fingerprints() {
        let (_, der1) = generate_self_signed_cert().unwrap();
        let (_, der2) = generate_self_signed_cert().unwrap();
        let fp1 = cert_fingerprint(&der1);
        let fp2 = cert_fingerprint(&der2);
        assert_ne!(fp1, fp2);
    }
}
