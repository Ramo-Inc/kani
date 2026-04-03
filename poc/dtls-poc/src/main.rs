//! DTLS over raw UDP Proof-of-Concept
//!
//! Validates that webrtc-rs's DTLS implementation can encrypt traffic over
//! raw UDP sockets on a LAN -- a prerequisite for Kani's input event transport.
//!
//! Usage:
//!   cargo run -- server
//!   cargo run -- client [server-ip]

use std::sync::Arc;
use std::time::Instant;

use ring::digest;
use webrtc_dtls::config::{ClientAuthType, Config, ExtendedMasterSecretType};
use webrtc_dtls::conn::DTLSConn;
use webrtc_dtls::crypto::{Certificate, CryptoPrivateKey};
use webrtc_util::conn::{Conn, Listener};

const PORT: u16 = 24900;
const MSG_COUNT: usize = 100;

/// Generate a self-signed certificate using rcgen, then convert to webrtc-dtls types.
fn generate_certificate() -> (Certificate, Vec<u8>) {
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .expect("Failed to generate key pair");

    let params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .expect("Failed to create cert params");

    let cert = params
        .self_signed(&key_pair)
        .expect("Failed to self-sign certificate");

    let cert_der = cert.der().to_vec();

    let private_key = CryptoPrivateKey::from_key_pair(&key_pair)
        .expect("Failed to convert key pair");

    let certificate = Certificate {
        certificate: vec![cert.into()],
        private_key,
    };

    (certificate, cert_der)
}

/// Compute SHA-256 fingerprint of a DER-encoded certificate.
fn fingerprint(der_bytes: &[u8]) -> String {
    let hash = digest::digest(&digest::SHA256, der_bytes);
    let hex: Vec<String> = hash.as_ref().iter().map(|b| format!("{b:02X}")).collect();
    format!("sha256:{}", hex.join(":"))
}

async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    let (cert, cert_der) = generate_certificate();
    println!("[server] Own fingerprint: {}", fingerprint(&cert_der));

    let config = Config {
        certificates: vec![cert],
        insecure_skip_verify: true,
        client_auth: ClientAuthType::RequireAnyClientCert,
        extended_master_secret: ExtendedMasterSecretType::Require,
        ..Default::default()
    };

    // Use webrtc-dtls listener which manages UDP internally
    let listener = webrtc_dtls::listener::listen(format!("0.0.0.0:{PORT}"), config).await?;
    println!("[server] Listening on 0.0.0.0:{PORT}");

    let (dtls_conn, peer_addr): (Arc<dyn Conn + Send + Sync>, _) =
        listener.accept().await?;
    println!("[server] Accepted DTLS connection from {peer_addr}");

    // Get peer certificate fingerprint via DTLSConn
    // The accept() returns Arc<dyn Conn>, but the underlying type is DTLSConn.
    // We need to downcast to get connection_state().
    if let Some(dtls) = dtls_conn.as_any().downcast_ref::<DTLSConn>() {
        let state = dtls.connection_state().await;
        if let Some(peer_cert) = state.peer_certificates.first() {
            println!(
                "[server] DTLS handshake complete. Peer fingerprint: {}",
                fingerprint(peer_cert)
            );
        } else {
            println!("[server] DTLS handshake complete. Peer fingerprint: none (no client cert)");
        }
    } else {
        println!("[server] DTLS handshake complete. (could not extract peer fingerprint)");
    }

    // Echo loop
    let mut buf = vec![0u8; 4096];
    loop {
        let n = match dtls_conn.recv(&mut buf).await {
            Ok(n) if n == 0 => break,
            Ok(n) => n,
            Err(e) => {
                println!("[server] Read error: {e}");
                break;
            }
        };
        if let Err(e) = dtls_conn.send(&buf[..n]).await {
            println!("[server] Write error: {e}");
            break;
        }
    }

    println!("[server] Connection closed.");
    listener.close().await?;
    Ok(())
}

async fn run_client(server_addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (cert, cert_der) = generate_certificate();
    println!("[client] Own fingerprint: {}", fingerprint(&cert_der));

    let config = Config {
        certificates: vec![cert],
        insecure_skip_verify: true,
        extended_master_secret: ExtendedMasterSecretType::Require,
        server_name: "localhost".to_string(),
        ..Default::default()
    };

    let dest = format!("{server_addr}:{PORT}");
    println!("[client] Connecting to {dest}...");

    // Create a connected UDP socket using webrtc_util's Conn-compatible type
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    sock.connect(&dest).await?;
    let sock: Arc<dyn Conn + Send + Sync> = Arc::new(sock);

    let hs_start = Instant::now();
    let dtls_conn = DTLSConn::new(sock, config, true, None).await?;
    let hs_ms = hs_start.elapsed().as_millis();

    // Get peer certificate fingerprint
    let state = dtls_conn.connection_state().await;
    let peer_fp = if let Some(peer_cert) = state.peer_certificates.first() {
        fingerprint(peer_cert)
    } else {
        "none".to_string()
    };
    println!("[client] DTLS handshake complete in {hs_ms}ms. Peer fingerprint: {peer_fp}");

    // Send test messages and measure RTT
    let dtls: Arc<dyn Conn + Send + Sync> = Arc::new(dtls_conn);
    let mut success = 0usize;
    let mut total_rtt = std::time::Duration::ZERO;
    let mut buf = vec![0u8; 4096];

    for i in 0..MSG_COUNT {
        let msg = format!("ping-{i:04}");
        let send_time = Instant::now();
        dtls.send(msg.as_bytes()).await?;

        match tokio::time::timeout(std::time::Duration::from_secs(5), dtls.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                let rtt = send_time.elapsed();
                total_rtt += rtt;
                let reply = String::from_utf8_lossy(&buf[..n]);
                if reply == msg {
                    success += 1;
                } else {
                    println!("[client] Mismatch on msg {i}: expected '{msg}', got '{reply}'");
                }
            }
            Ok(Err(e)) => println!("[client] Recv error on msg {i}: {e}"),
            Err(_) => println!("[client] Timeout on msg {i}"),
        }
    }

    let avg_rtt = if success > 0 {
        total_rtt.as_secs_f64() * 1000.0 / success as f64
    } else {
        0.0
    };

    println!("[client] {success}/{MSG_COUNT} messages echoed. Avg RTT: {avg_rtt:.2}ms");

    dtls.close().await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("server") => {
            if let Err(e) = run_server().await {
                eprintln!("[server] Error: {e}");
                std::process::exit(1);
            }
        }
        Some("client") => {
            let addr = args.get(2).map(|s| s.as_str()).unwrap_or("127.0.0.1");
            if let Err(e) = run_client(addr).await {
                eprintln!("[client] Error: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("Usage: dtls-poc <server|client> [server-ip]");
            std::process::exit(1);
        }
    }
}
