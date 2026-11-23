//! Live loopback TLS tests for [`WebPkiTransport::exchange`] covering the 24 MiB body cap,
//! 5-redirect hop limit, HTTPS->HTTP downgrade rejection, `POST`->`GET` 302 conversion, and
//! `Retry-After` parsing.

use super::net::{MAX_EXCHANGE_BYTES, Url, WebPkiTransport};
use super::tests::{TlsFixture, TlsReply};

#[test]
fn exchange_enforces_24_mib_body_cap_exact_and_plus_one() {
    let fixture = TlsFixture::start(|path, _| match path {
        "/exact" => {
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {MAX_EXCHANGE_BYTES}\r\nConnection: close\r\n\r\n"
            )
            .into_bytes();
            out.resize(out.len() + MAX_EXCHANGE_BYTES, b'a');
            TlsReply::Raw(out)
        }
        "/over" => {
            let over = MAX_EXCHANGE_BYTES + 1;
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {over}\r\nConnection: close\r\n\r\n"
            )
            .into_bytes();
            out.resize(out.len() + over, b'b');
            TlsReply::Raw(out)
        }
        _ => TlsReply::Raw(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()),
    });

    let trusted =
        WebPkiTransport::with_roots_for_test("Nightjar/test".into(), fixture.roots.clone());
    let exact_url = Url::parse(&fixture.url("/exact")).unwrap();
    let res = trusted.exchange("GET", &exact_url, &[], None, false).unwrap();
    assert_eq!(res.status, 200);
    assert_eq!(res.body.len(), MAX_EXCHANGE_BYTES);

    let over_url = Url::parse(&fixture.url("/over")).unwrap();
    let err = trusted
        .exchange("GET", &over_url, &[], None, false)
        .unwrap_err();
    assert_eq!(err, "response exceeded 24 MiB limit");
}

#[test]
fn exchange_follows_five_redirects_and_rejects_six() {
    let fixture = TlsFixture::start(|path, port| match path {
        "/r0" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{port}/r1\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        "/r1" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{port}/r2\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        "/r2" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{port}/r3\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        "/r3" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{port}/r4\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        "/r4" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{port}/r5\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        "/r5" => TlsReply::Raw(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nok5".to_vec()),
        "/s0" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{port}/r0\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        _ => TlsReply::Raw(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()),
    });

    let trusted =
        WebPkiTransport::with_roots_for_test("Nightjar/test".into(), fixture.roots.clone());
    let five_url = Url::parse(&fixture.url("/r0")).unwrap();
    let ok = trusted.exchange("GET", &five_url, &[], None, true).unwrap();
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body, b"ok5");

    let six_url = Url::parse(&fixture.url("/s0")).unwrap();
    let err = trusted
        .exchange("GET", &six_url, &[], None, true)
        .unwrap_err();
    assert_eq!(err, "too many redirects (5)");
}

#[test]
fn exchange_rejects_https_downgrade_converts_post_302_and_parses_retry_after() {
    let fixture = TlsFixture::start(|path, port| match path {
        "/downgrade" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/plain\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        "/post-redirect" => TlsReply::Raw(
            format!("HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:{port}/target\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        ),
        "/target" => TlsReply::Raw(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After:   7 \r\nContent-Length: 2\r\n\r\nhi"
                .to_vec(),
        ),
        "/bad-retry" => TlsReply::Raw(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\nContent-Length: 0\r\n\r\n"
                .to_vec(),
        ),
        _ => TlsReply::Raw(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()),
    });

    let trusted =
        WebPkiTransport::with_roots_for_test("Nightjar/test".into(), fixture.roots.clone());

    let downgrade_url = Url::parse(&fixture.url("/downgrade")).unwrap();
    assert_eq!(
        trusted
            .exchange("GET", &downgrade_url, &[], None, true)
            .unwrap_err(),
        "redirect downgraded off HTTPS"
    );

    let post_url = Url::parse(&fixture.url("/post-redirect")).unwrap();
    let res = trusted
        .exchange("POST", &post_url, &[], Some(b"payload"), true)
        .unwrap();
    assert_eq!(res.status, 429);
    assert_eq!(res.retry_after, Some(7));
    assert_eq!(res.body, b"hi");
    {
        let seen = fixture.seen_requests.lock().unwrap();
        let target_req = seen
            .iter()
            .find(|req| req.contains(" /target HTTP/1.1\r\n"))
            .expect("redirected request to /target");
        assert!(target_req.starts_with("GET /target HTTP/1.1\r\n"));
        assert!(!target_req.contains("Content-Length:"));
    }

    let bad_retry_url = Url::parse(&fixture.url("/bad-retry")).unwrap();
    let bad_res = trusted
        .exchange("GET", &bad_retry_url, &[], None, false)
        .unwrap();
    assert_eq!(bad_res.status, 429);
    assert_eq!(bad_res.retry_after, None);
}
