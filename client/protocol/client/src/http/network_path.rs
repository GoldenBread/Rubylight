//! The route to the host as the host sees it (`/api/network-path`), read as
//! NvHTTP.getNetworkPath and NetworkPathInfo.java do: only the current peer's route category,
//! no peer list or addresses. Gate on `Feature::NetworkPath`.

use super::{Error, Feature, HostApiError, HostCapabilities, HostClient};
use std::time::Duration;

pub use nightjar_policy::http::{NetworkPath, MAX_NETWORK_PATH_BYTES};

const CALL_TIMEOUT: Duration = Duration::from_secs(4);

fn invalid() -> HostApiError {
    HostApiError::Host(Error::Malformed("Invalid route response".into()))
}

impl HostClient {
    /// The route category to the host. `capabilities` comes from a paired serverinfo; without
    /// `Feature::NetworkPath` nothing is sent and the answer is [`HostApiError::Unavailable`].
    pub fn network_path(
        &self,
        capabilities: &HostCapabilities,
    ) -> Result<NetworkPath, HostApiError> {
        HostApiError::require(capabilities, Feature::NetworkPath)?;
        let response = self.pinned_get(
            "api/network-path",
            MAX_NETWORK_PATH_BYTES,
            CALL_TIMEOUT,
            true,
        )?;
        NetworkPath::parse(std::str::from_utf8(&response.body).map_err(|_| invalid())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::serverinfo::tests::RUBYLIGHT;
    use crate::http::{Availability, ServerInfo};

    #[test]
    fn accepts_only_known_categories_and_version() {
        // The cases of Android's NetworkPathInfoTest.
        for (json, path) in [
            (r#"{"version":1,"kind":"direct"}"#, NetworkPath::Direct),
            (r#"{"version":1,"kind":"relay"}"#, NetworkPath::Relay),
            (
                r#"{"version":1,"kind":"peer_relay"}"#,
                NetworkPath::PeerRelay,
            ),
            (r#"{"version":1,"kind":"future"}"#, NetworkPath::Unknown),
            (r#"{"version":1.0,"kind":"direct"}"#, NetworkPath::Direct),
            (r#"{"version":1,"kind":5}"#, NetworkPath::Unknown),
        ] {
            assert_eq!(NetworkPath::parse(json).unwrap(), path, "{json}");
        }
        for json in [
            "{}".to_owned(),
            "[]".to_owned(),
            "<html/>".to_owned(),
            r#"{"version":"1","kind":"direct"}"#.to_owned(),
            r#"{"version":1}"#.to_owned(),
            "\0".repeat(513),
            format!(
                r#"{{"version":1,"kind":"direct","pad":"{}"}}"#,
                " ".repeat(512)
            ),
        ] {
            assert!(
                matches!(
                    NetworkPath::parse(&json),
                    Err(HostApiError::Host(Error::Malformed(_)))
                ),
                "{json}"
            );
        }
        assert!(matches!(
            NetworkPath::parse(r#"{"version":2,"kind":"direct"}"#),
            Err(HostApiError::UnsupportedVersion)
        ));
    }

    #[test]
    fn a_host_without_the_feature_is_never_asked() {
        let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 1, identity)
            .unwrap()
            .with_https_port(1);
        client.pin(Some(client.identity().certificate_der().to_vec()));
        let without = ServerInfo::parse(
            &RUBYLIGHT.replace("<NetworkPathVersion>1</NetworkPathVersion>", ""),
            47989,
        )
        .unwrap()
        .capabilities;
        assert!(matches!(
            client.network_path(&without),
            Err(HostApiError::Unavailable {
                feature: Feature::NetworkPath,
                availability: Availability::Unsupported(_),
            })
        ));
        // With the feature the request goes out (and port 1 refuses it).
        let with = ServerInfo::parse(RUBYLIGHT, 47989).unwrap().capabilities;
        assert!(matches!(
            client.network_path(&with),
            Err(HostApiError::Host(Error::Io(_)))
        ));
    }
}
