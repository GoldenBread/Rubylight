//! Unified pairing, unpairing, PIN generation, certificate mismatch checks, and link-pairing
//! classification over [`HostClient`] and [`nightjar_policy::hosts::pairing`].

use nightjar_policy::hosts::{
    PairFailureKind, PairFailureState, UnpairFailureKind, classify_pairing_failure,
    classify_unpair,
};
use ring::rand::{SecureRandom, SystemRandom};
use rubylight_protocol::pairing_uri::PairingUri;

use crate::http::{Error, HostClient, PairFailure, PairResult, certificate_sha256_pem};

/// Cryptographically random 4-digit pairing PIN (`"0000"`..`"9999"`).
pub fn random_pin() -> String {
    let mut bytes = [0u8; 4];
    if SystemRandom::new().fill(&mut bytes).is_ok() {
        let val = u32::from_le_bytes(bytes) % 10_000;
        return format!("{val:04}");
    }
    "0000".into()
}

/// Whether `server_certificate_pem` is present and pins a certificate whose SHA-256 differs from
/// `certificate_sha256` (unparseable PEM is treated as a different certificate).
pub fn pins_another_pem(
    server_certificate_pem: Option<&str>,
    certificate_sha256: &[u8; 32],
) -> bool {
    server_certificate_pem.is_some_and(|pem| certificate_sha256_pem(pem).as_ref() != Some(certificate_sha256))
}

/// Secret material for pairing a host client.
#[derive(Clone, Debug)]
pub enum PairSecret {
    Pin(String),
    OneTime { pin: String, passphrase: String },
    Link(Box<PairingUri>),
}

/// Classifies a completed [`PairResult`] into `Ok(pem)` or typed [`PairFailureKind`].
pub fn pair_outcome_kind(
    result: PairResult,
    from_link: bool,
    one_time: bool,
) -> Result<String, PairFailureKind> {
    match result {
        PairResult::Paired {
            server_certificate_pem,
        } => Ok(server_certificate_pem),
        PairResult::PinWrong => Err(classify_pairing_failure(
            PairFailureState::PinWrong,
            from_link,
            one_time,
            false,
        )),
        PairResult::AlreadyInProgress => Err(PairFailureKind::AlreadyInProgress),
        PairResult::Failed(PairFailure::ServerSignature) => Err(PairFailureKind::ServerUnverified),
        PairResult::Failed(PairFailure::Declined(_)) => Err(PairFailureKind::Declined),
    }
}

/// Maps an [`Error`] from `pair` into either a fixed [`PairFailureKind`] or `None` when callers
/// should surface [`Error::to_string`] directly.
pub fn pair_error_kind(error: &Error) -> Option<PairFailureKind> {
    match error {
        Error::Io(_) => Some(PairFailureKind::Network),
        Error::CertificateMismatch => Some(PairFailureKind::LinkCertMismatch),
        _ => None,
    }
}

/// Classifies an unpair [`Error`] into [`UnpairFailureKind`].
pub fn unpair_error_kind(error: &Error) -> UnpairFailureKind {
    match error {
        Error::NotPaired => classify_unpair(None, true, false, false),
        Error::NotFound(_) => classify_unpair(Some(404), false, false, true),
        Error::Status { code, .. } => classify_unpair(Some(*code as u16), false, false, false),
        _ => classify_unpair(None, false, false, false),
    }
}

/// Executes plain HTTP `serverinfo` followed by the chosen [`PairSecret`] pairing handshake.
pub fn execute_pair(
    client: &HostClient,
    secret: &PairSecret,
    known_otherwise: &[String],
) -> Result<String, Result<PairFailureKind, Error>> {
    let info = client.server_info_http(true).map_err(|e| {
        pair_error_kind(&e).map(Ok).unwrap_or(Err(e))
    })?;
    if matches!(secret, PairSecret::Link(_)) && known_otherwise.contains(&info.unique_id) {
        return Err(Ok(PairFailureKind::LinkCertMismatch));
    }
    let from_link = matches!(secret, PairSecret::Link(_));
    let one_time = !matches!(secret, PairSecret::Pin(_));
    let res = match secret {
        PairSecret::Pin(pin) => client.pair(&info, pin),
        PairSecret::OneTime { pin, passphrase } => {
            client.pair_with_passphrase(&info, pin, passphrase)
        }
        PairSecret::Link(link) => client.pair_with_link(&info, link),
    }
    .map_err(|e| pair_error_kind(&e).map(Ok).unwrap_or(Err(e)))?;
    pair_outcome_kind(res, from_link, one_time).map_err(Ok)
}

/// Attempts unpair on `client` and treats `401` / `NotPaired` as already unpaired.
pub fn execute_unpair(client: &HostClient) -> Result<(), UnpairFailureKind> {
    match client.unpair() {
        Ok(()) => Ok(()),
        Err(err) => {
            let kind = unpair_error_kind(&err);
            if kind == UnpairFailureKind::NotPaired {
                Ok(())
            } else {
                Err(kind)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_pin_is_four_ascii_digits() {
        for _ in 0..10 {
            let pin = random_pin();
            assert_eq!(pin.len(), 4);
            assert!(pin.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn pair_and_unpair_classifications_match_policy() {
        assert_eq!(
            pair_outcome_kind(PairResult::PinWrong, false, true),
            Err(PairFailureKind::OneTimePinWrong)
        );
        assert_eq!(
            pair_outcome_kind(PairResult::PinWrong, false, false),
            Err(PairFailureKind::PinWrong)
        );
        assert_eq!(
            pair_outcome_kind(PairResult::PinWrong, true, true),
            Err(PairFailureKind::LinkPinWrong)
        );
        assert_eq!(
            unpair_error_kind(&Error::Status {
                code: 401,
                message: "Unauthorized".into()
            }),
            UnpairFailureKind::NotPaired
        );
        assert_eq!(
            unpair_error_kind(&Error::Io(std::io::ErrorKind::TimedOut.into())),
            UnpairFailureKind::Failed
        );
    }
}
