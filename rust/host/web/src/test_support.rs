//! Test helpers for exercising the web console and HTTP API routers without a socket listener.
use axum::Extension;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use butterpollo_session::state::Credentials;
use butterpollo_stream::state::test_support::Fixture;
use serde_json::Value;
use tower::ServiceExt;

/// Send a JSON HTTP request as the `parity` administrator to [`crate::web::router`].
pub async fn request(f: &Fixture, method: &str, path: &str, data: Value) -> Response {
    request_with(f, method, path, data, &[]).await
}

/// [`request`] with extra request headers.
pub async fn request_with(
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
    crate::web::router(f.host.clone())
        .layer(Extension(f.connection(true)))
        .oneshot(
            builder
                .body(Body::from(serde_json::to_vec(&data).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Assert `response` status matches `status` and parse its JSON body.
pub async fn value(response: Response, status: StatusCode) -> Value {
    assert_eq!(response.status(), status);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
