//! Self-signed TLS certificate generation.

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose, SanType};

use crate::error::{Error, Result};

/// Generate a self-signed certificate and key with `SAN = IP:<server_ip>`.
///
/// The certificate is written world-readable; the key is written mode `0600`.
/// It carries `basicConstraints CA:TRUE` so OpenSSL (and therefore Python's
/// `ssl` module) accepts it as a pinned trust anchor while it is also the
/// server certificate. Returns the certificate and key paths.
pub fn generate(state_dir: &Path, server_ip: IpAddr) -> Result<(PathBuf, PathBuf)> {
    let cert_path = state_dir.join("server.crt");
    let key_path = state_dir.join("server.key");

    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "lanpull");
    let mut sans = vec![
        SanType::IpAddress(server_ip),
        SanType::IpAddress(IpAddr::from([127, 0, 0, 1])),
    ];
    if let Ok(localhost) = rcgen::string::Ia5String::try_from("localhost") {
        sans.push(SanType::DnsName(localhost));
    }
    params.subject_alt_names = sans;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
        KeyUsagePurpose::KeyCertSign,
    ];

    let key_pair = KeyPair::generate().map_err(|e| Error::Certificate(e.to_string()))?;
    let certificate = params
        .self_signed(&key_pair)
        .map_err(|e| Error::Certificate(e.to_string()))?;

    let cert_pem = certificate.pem();
    let key_pem = key_pair.serialize_pem();

    fs::create_dir_all(state_dir)?;
    crate::atomic::write(&cert_path, cert_pem.as_bytes())?;
    crate::atomic::write_private(&key_path, key_pem.as_bytes())?;

    Ok((cert_path, key_path))
}
