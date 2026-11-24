//! NVHTTP router parity, serverinfo, pairing, launch, bitrate, clipboard, and role tests.
use anyhow::Result;
use axum::{
    Extension,
    body::{Body, to_bytes},
    extract::{Query, State},
    http::{Request, StatusCode, header},
    response::Response,
};
use butterpollo_crypto::crypto;
use butterpollo_session::power_action::SleepStates;
use butterpollo_session::remote::{self, Control};
use butterpollo_session::session::Role;
use butterpollo_session::state::{App, Client};
use butterpollo_stream::nvhttp::pairing::do_pair;
use butterpollo_stream::nvhttp::test_support::SESSION_ERROR;
use butterpollo_stream::nvhttp::*;
use butterpollo_stream::state::{OneTimePin, PendingPin, test_support::Fixture};
use butterpollo_stream::tls::Connection;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tower::ServiceExt;

async fn document(response: Response) -> xmltree::Element {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/xml")
    );
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    xmltree::Element::parse(bytes.as_ref()).unwrap()
}
fn field(root: &xmltree::Element, name: &str) -> String {
    root.get_child(name)
        .unwrap()
        .get_text()
        .unwrap_or_default()
        .into_owned()
}
async fn request(f: &Fixture, https: bool, method: &str, path: &str) -> Response {
    router(f.host.clone(), https)
        .layer(Extension(f.connection(https)))
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// A certificate request from `client`, named `id`, with the one-time PIN
/// hash of `pin` and `passphrase` when given.
pub(super) fn device_request(
    client: &crypto::Identity,
    id: &str,
    otp: Option<(&str, &str)>,
) -> Args {
    let mut args = Args::from([
        ("uniqueid".into(), id.into()),
        ("phrase".into(), "getservercert".into()),
        ("salt".into(), hex::encode([0x12; 16])),
        (
            "clientcert".into(),
            hex::encode(client.certificate.as_bytes()),
        ),
        ("devicename".into(), id.into()),
    ]);
    if let Some((pin, passphrase)) = otp {
        args.insert(
            "otpauth".into(),
            hex::encode(crypto::hash(
                format!("{pin}{}{passphrase}", hex::encode([0x12; 16])).as_bytes(),
            )),
        );
    }
    args
}
/// The rest of `client`'s handshake after its certificate request, with `pin`.
pub(super) async fn finish_pairing(
    f: &Fixture,
    client: &crypto::Identity,
    id: &str,
    pin: &str,
) -> Result<()> {
    let peer = "127.0.0.1".parse().unwrap();
    let aes = crypto::pin_key(&[0x12; 16], pin);
    let step = |name: &str, value: Vec<u8>| {
        Args::from([
            ("uniqueid".into(), id.into()),
            (name.into(), hex::encode(value)),
        ])
    };
    let challenge = crypto::ecb(&aes, &[0x34; 16], true)?;
    let response = do_pair(f.host.clone(), &step("clientchallenge", challenge), peer).await?;
    let response = crypto::ecb(&aes, &hex::decode(&response[1].1)?, false)?;
    let secret = [0x56; 16];
    let proof = crypto::hash(&[&response[32..], &client.signature, &secret].concat());
    let proof = crypto::ecb(&aes, &proof, true)?;
    do_pair(f.host.clone(), &step("serverchallengeresp", proof), peer).await?;
    let secret = [&secret[..], &client.sign(&secret)].concat();
    do_pair(f.host.clone(), &step("clientpairingsecret", secret), peer).await?;
    Ok(())
}

pub(super) fn one_time_pin(pin: &str, passphrase: &str, permission: Option<u32>) -> OneTimePin {
    OneTimePin {
        pin: pin.into(),
        passphrase: passphrase.into(),
        device_name: String::new(),
        created: Instant::now(),
        misses: Default::default(),
        permission,
    }
}

#[path = "parity_tests/applist_and_launch_tests.rs"]
mod applist_and_launch_tests;
#[path = "parity_tests/bitrate_and_clipboard_tests.rs"]
mod bitrate_and_clipboard_tests;
#[path = "parity_tests/metrics_and_serverinfo_tests.rs"]
mod metrics_and_serverinfo_tests;
#[path = "parity_tests/pairing_tests.rs"]
mod pairing_tests;
#[path = "parity_tests/roles_and_mic_tests.rs"]
mod roles_and_mic_tests;
#[path = "parity_tests/serverinfo_snapshot_and_modes_tests.rs"]
mod serverinfo_snapshot_and_modes_tests;
