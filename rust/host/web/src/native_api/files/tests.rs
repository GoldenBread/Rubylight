//! Unit and property tests for Native API file transfer framing, pacing, quotas, and decoding.
use super::download::read_exactly;
use super::*;

fn send(file: &[u8], length: u64) -> (io::Result<()>, Vec<u8>) {
    let mut sent = Vec::new();
    let result = read_exactly(&mut io::Cursor::new(file), length, |chunk| {
        sent.extend_from_slice(&chunk);
        Ok(true)
    });
    (result, sent)
}

#[test]
fn an_elevated_host_without_a_shell_says_how_to_reach_the_files() {
    let refused = failure("start the upload", UserFolderError::NoUserShell);
    assert_eq!(refused.code(), ErrorCode::Internal);
    assert!(
        refused.message().contains("administrator"),
        "{}",
        refused.message()
    );
    assert!(
        refused.message().contains("Windows service"),
        "{}",
        refused.message()
    );
}

#[test]
fn a_download_sends_exactly_its_length_or_fails_before_the_last_byte() {
    let file: Vec<u8> = (0..CHUNK * 2 + 7).map(|i| i as u8).collect();
    let (result, sent) = send(&file, file.len() as u64);
    result.unwrap();
    assert_eq!(sent, file);
    assert!(send(&[], 0).0.is_ok());
    // Grown since it was opened: the last chunk is held back.
    let (result, sent) = send(&file, file.len() as u64 - 1);
    assert!(result.unwrap_err().to_string().contains("grew"));
    assert!(sent.len() < file.len() - 1);
    assert!(send(&[1], 0).0.is_err());
    // Shrunk: everything there is goes, then the error ends the body.
    let (result, sent) = send(&file, file.len() as u64 + 1);
    assert!(result.unwrap_err().to_string().contains("shrank"));
    assert_eq!(sent, file);
}

#[test]
fn a_client_gone_stops_the_download_quietly_and_a_refusal_with_its_error() {
    let file = vec![0; CHUNK * 3];
    let mut calls = 0;
    let result = read_exactly(&mut io::Cursor::new(&file), file.len() as u64, |_| {
        calls += 1;
        Ok(false)
    });
    assert!(result.is_ok());
    assert_eq!(calls, 1);
    let result = read_exactly(&mut io::Cursor::new(&file), file.len() as u64, |_| {
        Err(io::Error::from(io::ErrorKind::TimedOut))
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
}

fn set(h: &Shared, key: &str, value: &str) {
    h.config
        .write()
        .unwrap()
        .values
        .insert(key.into(), value.into());
}
/// Makes the pacer look at the stream and the settings again at once,
/// as it would after [`PACE_CHECK`].
fn look_again(h: &Shared) {
    h.file_transfers.clear_pacing_check_for_test();
}

#[test]
fn transfers_share_one_pace_and_only_while_a_stream_runs() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    set(h, "file_transfer_stream_mbps", "1");
    let pace = |bytes| h.file_transfers.pace(h, bytes);
    assert!(pace(PACE_BURST * 4).is_zero(), "no stream: unpaced");
    f.session(f.client(0), butterpollo_session::session::Role::Stream);
    look_again(h);
    // Two transfers, one budget: the second waits for what the first took.
    let first = pace(PACE_BURST);
    let second = pace(125_000);
    assert!(first < Duration::from_millis(100), "{first:?}");
    assert!(second >= Duration::from_millis(900), "{second:?}");
}

#[test]
fn a_new_stream_rate_applies_to_the_transfers_running() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    f.session(f.client(0), butterpollo_session::session::Role::Stream);
    let rate = || h.file_transfers.pacer_rate_for_test();
    h.file_transfers.pace(h, 1);
    assert_eq!(rate(), Some(50 * 125_000), "the default 50 Mbit/s");
    set(h, "file_transfer_stream_mbps", "8");
    // Not before the next look, which comes every PACE_CHECK.
    h.file_transfers.pace(h, 1);
    assert_eq!(rate(), Some(50 * 125_000));
    look_again(h);
    h.file_transfers.pace(h, 1);
    assert_eq!(rate(), Some(1_000_000));
    set(h, "file_transfer_stream_mbps", "0");
    look_again(h);
    assert!(h.file_transfers.pace(h, 100 << 20).is_zero());
    assert_eq!(rate(), None);
}

#[test]
fn an_upload_leaves_the_reserve_and_the_bytes_running_uploads_still_write() {
    use std::collections::HashMap;
    const GIB: u64 = 1 << 30;
    let none = HashMap::new();
    // 1 GiB on a small volume, 2 % on a mid-sized one, 4 GiB on a big one.
    assert_eq!(room(&none, 10 * GIB, 20 * GIB), 9 * GIB);
    assert_eq!(room(&none, 30 * GIB, 100 * GIB), 28 * GIB);
    assert_eq!(room(&none, 30 * GIB, 1000 * GIB), 26 * GIB);
    // A 930 GB drive with 15 GB free, where 2 % would leave nothing.
    assert_eq!(
        room(&none, 15_000_000_000, 930_000_000_000),
        15_000_000_000 - 4 * GIB
    );
    assert_eq!(room(&none, 3 * GIB, 4000 * GIB), 0);
    assert_eq!(room(&none, GIB / 2, 20 * GIB), 0);
    let running = HashMap::from([("a".to_owned(), 3 * GIB), ("b".to_owned(), GIB)]);
    assert_eq!(room(&running, 10 * GIB, 20 * GIB), 5 * GIB);
    assert_eq!(room(&running, 4 * GIB, 20 * GIB), 0);
    let huge = HashMap::from([("a".to_owned(), u64::MAX), ("b".to_owned(), u64::MAX)]);
    assert_eq!(room(&huge, u64::MAX, u64::MAX), 0);
}

#[test]
fn names_are_percent_decoded_and_must_be_utf8_and_single() {
    assert_eq!(name_of(Some("name=a%20b%2Bc.txt")).unwrap(), "a b+c.txt");
    assert_eq!(name_of(Some("x=1&name=%C3%A4.png")).unwrap(), "ä.png");
    // RFC 3986: a plus is a plus, not a space.
    assert_eq!(name_of(Some("name=a+b")).unwrap(), "a+b");
    assert_eq!(name_of(Some("name=C%2B%2B+x.txt")).unwrap(), "C+++x.txt");
    for query in [
        None,
        Some(""),
        Some("name=%FF"),
        Some("name=%2"),
        Some("name=%+A"),
        Some("name=a&name=b"),
    ] {
        let failure = name_of(query).unwrap_err();
        assert_eq!(failure.code, ErrorCode::InvalidName, "{query:?}");
    }
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    /// RFC 3986 percent-decoding: `%` and exactly two hex digits.
    fn reference(value: &str) -> Option<String> {
        let mut bytes = vec![];
        let mut rest = value.as_bytes();
        while let Some((&byte, tail)) = rest.split_first() {
            rest = tail;
            if byte != b'%' {
                bytes.push(byte);
                continue;
            }
            let digit = |at: usize| (*rest.get(at)? as char).to_digit(16);
            bytes.push((digit(0)? * 16 + digit(1)?) as u8);
            rest = &rest[2..];
        }
        String::from_utf8(bytes).ok()
    }
    fn encode(value: &str) -> String {
        value
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    char::from(b).to_string()
                }
                b => format!("%{b:02X}"),
            })
            .collect()
    }

    proptest! {
        /// Any query value decodes as RFC 3986 has it; a sign after `%` is not a digit.
        #[test]
        fn values_decode_as_rfc_3986_has_it(value in r"([%+a-fA-F0-9 .\-]|%[+-]?[0-9a-fA-F]{0,2}|\PC){0,24}") {
            prop_assert_eq!(decode(&value), reference(&value));
        }

        #[test]
        fn encoded_names_round_trip(name in r"\PC{0,40}", before in "[a-z=&]{0,8}") {
            let query = format!("{before}&name={}", encode(&name));
            let decoded = name_of(Some(&query));
            if before.split('&').any(|pair| pair.split_once('=').map_or(pair, |(k, _)| k) == "name") {
                prop_assert!(decoded.is_err(), "two names");
            } else {
                prop_assert_eq!(decoded.ok(), Some(name));
            }
        }
    }
}
