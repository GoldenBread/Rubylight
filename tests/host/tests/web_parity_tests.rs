//! Web console API/auth/config parity tests extracted from `butterpollo_web::web`.
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use butterpollo_crypto::auth;
use butterpollo_session::{session::Role, state, state::Credentials};
use butterpollo_stream::state::{PendingPin, test_support::Fixture};
use butterpollo_web::web::router;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tower::ServiceExt;

async fn request(f: &Fixture, method: &str, path: &str, data: Value) -> Response {
    request_with(f, method, path, data, &[]).await
}
/// [`request`] with extra headers.
async fn request_with(
    f: &Fixture,
    method: &str,
    path: &str,
    data: Value,
    headers: &[(&str, &str)],
) -> Response {
    // On disk too: the host takes up the sign-in the file holds.
    let credentials = Credentials::new("parity".into(), "parity-test").unwrap();
    f.host.save_credentials(&credentials).unwrap();
    *f.host.credentials.write().unwrap() = Some(credentials);
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, "Basic cGFyaXR5OnBhcml0eS10ZXN0")
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            builder
                .body(Body::from(serde_json::to_vec(&data).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn value(response: Response, status: StatusCode) -> Value {
    assert_eq!(response.status(), status);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[path = "web_parity_tests/usb_tests.rs"]
mod usb_tests;

#[path = "web_parity_tests/pairing_and_library_tests.rs"]
mod pairing_and_library_tests;
#[path = "web_parity_tests/permissions_and_auth_tests.rs"]
mod permissions_and_auth_tests;
