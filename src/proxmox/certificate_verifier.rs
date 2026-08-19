use crate::error::PwpError;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use sha2::{Digest, Sha256};
use std::fmt::Debug;
use std::sync::Arc;
use webpki::{EndEntityCert, KeyUsage};

#[derive(Debug)]
pub struct PwpProxmoxCertificateVerifier {
    expected_fingerprint: Vec<u8>,
}

impl PwpProxmoxCertificateVerifier {
    pub fn new(certificate_fingerprint: &str) -> Result<Arc<Self>, PwpError> {
        let expected_fingerprint =
            hex::decode(certificate_fingerprint.replace(":", "").to_lowercase())
                .map_err(PwpError::InvalidCertificateFingerprint)?;

        Ok(Arc::new(Self {
            expected_fingerprint,
        }))
    }
}

impl ServerCertVerifier for PwpProxmoxCertificateVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let fingerprint = Sha256::digest(end_entity.as_ref());

        if fingerprint.as_slice() != self.expected_fingerprint.as_slice() {
            return Err(Error::General("SHA-256 fingerprint mismatch".into()));
        }

        let parsed_certificate = EndEntityCert::try_from(end_entity)
            .map_err(|_| Error::General("Failed to parse certificate".into()))?;

        parsed_certificate
            .verify_is_valid_for_subject_name(server_name)
            .map_err(|_| {
                Error::General("Certificate is not valid for the given server name".into())
            })?;

        if let Ok(anchor) = webpki::anchor_from_trusted_cert(end_entity) {
            let supported_schemes = CryptoProvider::get_default()
                .expect("No default crypto provider set")
                .signature_verification_algorithms;

            parsed_certificate
                .verify_for_usage(
                    supported_schemes.all,
                    &[anchor],
                    &[],
                    now,
                    KeyUsage::server_auth(),
                    None,
                    None,
                )
                .map_err(|_| {
                    Error::General(
                        "Certificate is expired, not yet valid or not valid for this usage".into(),
                    )
                })?;
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        let supported_schemes = CryptoProvider::get_default()
            .expect("No default crypto provider set")
            .signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, cert, dss, &supported_schemes)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        let supported_schemes = CryptoProvider::get_default()
            .expect("No default crypto provider set")
            .signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, cert, dss, &supported_schemes)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        CryptoProvider::get_default()
            .expect("No default crypto provider set")
            .signature_verification_algorithms
            .supported_schemes()
    }
}
