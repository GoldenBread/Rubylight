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

/// Cached parity credentials so test helpers don't re-run 600k-round PBKDF2 per request.
static PARITY_CREDENTIALS: std::sync::OnceLock<Credentials> = std::sync::OnceLock::new();

/// [`request`] with extra request headers.
pub async fn request_with(
    f: &Fixture,
    method: &str,
    path: &str,
    data: Value,
    headers: &[(&str, &str)],
) -> Response {
    // On disk too: the host takes up the sign-in the file holds (unless already parity).
    let already_parity = f
        .host
        .credentials
        .read()
        .unwrap()
        .as_ref()
        .is_some_and(|c| c.username == "parity");
    if !already_parity {
        let credentials = PARITY_CREDENTIALS
            .get_or_init(|| Credentials::new("parity".into(), "parity-test").unwrap())
            .clone();
        f.host.save_credentials(&credentials).unwrap();
        *f.host.credentials.write().unwrap() = Some(credentials);
    }
    let has_auth = headers.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case(header::AUTHORIZATION.as_str())
            || name.eq_ignore_ascii_case(header::COOKIE.as_str())
    });
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if !has_auth {
        let (access, _, csrf, _) = f
            .host
            .new_web_session(
                "parity".into(),
                false,
                "test".into(),
                "127.0.0.1".into(),
                None,
            )
            .unwrap();
        builder = builder
            .header(header::COOKIE, format!("__Host-apollo_session={access}"))
            .header("X-CSRF-Token", csrf);
    }
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
