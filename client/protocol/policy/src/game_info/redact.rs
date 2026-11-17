//! Keeping the player's keys and the IGDB token out of logs.

/// What stands in for a secret.
pub const MASK: &str = "[redacted]";

/// `text` with every one of `secrets` (empty ones and ones under 4 characters left alone, as
/// they would mask ordinary words) replaced by [`MASK`], and with the value of any
/// `client_secret=`, `client_id=`, `access_token` or `Bearer` it holds masked too, for a secret
/// the caller did not know.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_owned();
    let mut sorted: Vec<&str> = secrets.iter().copied().filter(|s| s.trim().len() >= 4).collect();
    // The longer first, so one holding another is masked whole.
    sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in sorted {
        out = out.replace(secret, MASK);
    }
    for marker in ["client_secret=", "client_id=", "access_token=", "Bearer ", "\"access_token\":\"", "\"access_token\": \""] {
        out = mask_after(&out, marker);
    }
    out
}

/// `text` with the value after each `marker` (up to `&`, a quote, a space or the end) masked.
fn mask_after(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let start = at + marker.len();
        out.push_str(&rest[..start]);
        let value_len = rest[start..].find(['&', '"', '\'', ' ', ',', '\n', '}']).unwrap_or(rest.len() - start);
        if value_len > 0 && !rest[start..start + value_len].starts_with(MASK) {
            out.push_str(MASK);
        } else {
            out.push_str(&rest[start..start + value_len]);
        }
        rest = &rest[start + value_len..];
    }
    out.push_str(rest);
    out
}
