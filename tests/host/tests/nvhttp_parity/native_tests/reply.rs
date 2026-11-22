//! Shared HTTP reply assertion helper and secondary paired-device builder for `native_tests`.
use super::{ApiSession, Client, Connection, Identity, Setup};
use axum::http::{HeaderMap, StatusCode, header};
pub(super) use butterpollo_stream::stream::reconnect_harness::HOST_VERSION;
use serde_json::Value;

pub(super) struct Reply {
    pub(super) status: StatusCode,
    pub(super) headers: HeaderMap,
    pub(super) body: Vec<u8>,
}

impl Reply {
    pub(super) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub(super) fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }

    pub(super) fn session(&self) -> ApiSession {
        assert_eq!(self.status, StatusCode::OK, "{}", self.text());
        serde_json::from_slice(&self.body).unwrap()
    }

    /// The answer is the documented error: status, shape and headers.
    pub(super) fn assert_error(&self, status: u16, code: &str) {
        assert_eq!(self.status.as_u16(), status, "{}", self.text());
        let body = self.json();
        assert_eq!(body["error"]["code"], code, "{body}");
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());
        for field in ["setting", "retry_after_ms"] {
            assert!(
                body["error"].get(field).is_some(),
                "{field} is null, not missing"
            );
        }
        assert_eq!(
            self.headers[header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert_eq!(self.headers[header::CACHE_CONTROL], "no-store");
    }
}

/// Another paired device, by name.
pub(super) fn paired_device(s: &Setup, uuid: &str, name: &str, perm: u32) -> (Client, Connection) {
    let identity = Identity::generate().unwrap();
    let client = Client {
        name: name.into(),
        cert: identity.certificate.clone(),
        uuid: uuid.into(),
        perm,
        enabled: true,
        extra: Default::default(),
    };
    s.host()
        .paired
        .write()
        .unwrap()
        .add(&s.host().paired_path, client.clone())
        .unwrap();
    let connection = Connection {
        certificate: Some(identity.der.clone()),
        ..s.f.connection(true)
    };
    (client, connection)
}
