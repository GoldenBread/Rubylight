//! Pinned-certificate TLS configuration and rustls error mapping for host HTTP.

use super::super::Error;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use std::sync::Arc;

pub(crate) fn tls_error(error: rustls::Error) -> Error {
    match error {
        rustls::Error::InvalidCertificate(_) => Error::CertificateMismatch,
        other => Error::Tls(other.to_string()),
    }
}

pub(crate) fn io_tls_error(error: std::io::Error) -> Error {
    let inner = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
        .cloned();
    match inner {
        Some(e) => tls_error(e),
        None => Error::Io(error),
    }
}

/// Accepts exactly the pinned certificate, whatever its name or issuer.
#[derive(Debug)]
struct PinnedCertificate {
    pinned: Vec<u8>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.pinned.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// TLS that presents the client certificate and trusts only `server_certificate`.
pub(crate) fn tls_config(
    server_certificate: &[u8],
    client_certificate: &[u8],
    client_key_pkcs8: &[u8],
) -> Result<Arc<ClientConfig>, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinnedCertificate {
        pinned: server_certificate.to_vec(),
        provider: provider.clone(),
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(
            vec![CertificateDer::from(client_certificate.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(client_key_pkcs8.to_vec())),
        )
        .map_err(tls_error)?;
    // Sunshine and its forks (Vibepollo) identify the paired client in their certificate
    // callback, which a resumed session skips: they answer it with internal_error (#39).
    // Moonlight never resumes either, so every request does a full handshake.
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}
