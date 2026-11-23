//! Pure update scheduling, release offering and Markdown release-notes sanitization shared
//! across Nightjar Android and Nightjar Desktop.

pub mod release;
pub mod semver;

pub use release::{
    ChecksumFile, DesktopAsset, DesktopAssetError, Release, ReleaseAsset, is_safe_asset_name,
};
pub use semver::{SemVer, is_strict_stable_tag, newer_stable_tag};

pub const CHECK_INTERVAL_MS: i64 = 12 * 60 * 60 * 1000;
/// Store package that manages updates itself; the self-updater stays out of its way.
pub const PLAY_STORE: &str = "com.android.vending";

/// Pure policy decisions for when to check and whether to show an update offer.
pub struct UpdatePolicy;

impl UpdatePolicy {
    /// An automatic check runs when enabled and none ran in the last 12 hours (or clock went back).
    pub fn is_check_due(auto_check: bool, last_check_ms: i64, now_ms: i64) -> bool {
        if !auto_check {
            return false;
        }
        last_check_ms <= 0 || now_ms < last_check_ms || now_ms - last_check_ms >= CHECK_INTERVAL_MS
    }

    /// Whether an automatic check should offer `release` given `current_version` and `skipped_version`.
    pub fn should_offer(
        release: Option<&Release>,
        current_version: &str,
        skipped_version: Option<&str>,
    ) -> bool {
        let Some(release) = release else {
            return false;
        };
        if !release.is_update_for(current_version) {
            return false;
        }
        match skipped_version {
            None | Some("") => true,
            Some(skipped) => SemVer::normalize(skipped) != release.version(),
        }
    }

    pub fn is_managed_by_store(installer_package: Option<&str>) -> bool {
        installer_package == Some(PLAY_STORE)
    }

    pub fn is_self_update_available(
        installer_package: Option<&str>,
        github_distribution: bool,
    ) -> bool {
        github_distribution && !Self::is_managed_by_store(installer_package)
    }
}

/// Turns a release's Markdown body into short plain text for the update dialog, truncating on
/// UTF-16 code-unit boundaries so Android surrogate pairs are never split.
pub struct ReleaseNotes;

impl ReleaseNotes {
    pub fn plain(markdown: &str, max_chars: usize) -> String {
        let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
        let stripped = strip_images_links_and_html(&normalized);
        let mut lines_out = Vec::new();
        for raw_line in stripped.split('\n') {
            let mut line = raw_line;
            // Up to 3 spaces/tabs followed by 1..=6 '#' and optional spaces/tabs.
            let leading_ws = line
                .bytes()
                .take(3)
                .take_while(|&b| b == b' ' || b == b'\t')
                .count();
            let after_ws = &line[leading_ws..];
            let hashes = after_ws.bytes().take(6).take_while(|&b| b == b'#').count();
            if hashes > 0 && after_ws.as_bytes().get(hashes) != Some(&b'#') {
                let after_hashes = &after_ws[hashes..];
                let trailing_ws = after_hashes
                    .bytes()
                    .take_while(|&b| b == b' ' || b == b'\t')
                    .count();
                line = &after_hashes[trailing_ws..];
            }
            // Quote marker `[ \t]*>[ \t]?`.
            let quote_ws = line
                .bytes()
                .take_while(|&b| b == b' ' || b == b'\t')
                .count();
            if line.as_bytes().get(quote_ws) == Some(&b'>') {
                let after_q = &line[quote_ws + 1..];
                line = after_q
                    .strip_prefix(' ')
                    .or_else(|| after_q.strip_prefix('\t'))
                    .unwrap_or(after_q);
            }
            // Bullet `^([ \t]*)[-*+][ \t]+` -> `$1• `.
            let bullet_ws = line
                .bytes()
                .take_while(|&b| b == b' ' || b == b'\t')
                .count();
            let rest = &line[bullet_ws..];
            let owned = if rest.starts_with(['-', '*', '+'])
                && rest
                    .as_bytes()
                    .get(1)
                    .is_some_and(|&b| b == b' ' || b == b'\t')
            {
                let after_bullet = &rest[1..];
                let gap = after_bullet
                    .bytes()
                    .take_while(|&b| b == b' ' || b == b'\t')
                    .count();
                format!("{}• {}", &line[..bullet_ws], &after_bullet[gap..])
            } else {
                line.to_owned()
            };
            let clean = owned
                .replace("**", "")
                .replace("__", "")
                .replace("~~", "")
                .replace('`', "");
            lines_out.push(clean);
        }
        let joined = lines_out.join("\n");
        let collapsed = collapse_blank_runs(&joined);
        let text = collapsed.trim();
        let utf16: Vec<u16> = text.encode_utf16().collect();
        if utf16.len() > max_chars {
            let mut cut = max_chars.saturating_sub(1);
            if cut > 0 && (0xD800..=0xDBFF).contains(&utf16[cut - 1]) {
                cut -= 1;
            }
            let prefix = String::from_utf16_lossy(&utf16[..cut]);
            format!("{}…", prefix.trim())
        } else {
            text.to_owned()
        }
    }
}

fn strip_images_links_and_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'!'
            && bytes.get(i + 1) == Some(&b'[')
            && let Some(end) = match_md_link(input, i + 1)
        {
            i = end.1;
            continue;
        }
        if bytes[i] == b'['
            && let Some((label, end)) = match_md_link(input, i)
        {
            out.push_str(label);
            i = end;
            continue;
        }
        if bytes[i] == b'<'
            && let Some(rel_close) = input[i + 1..].find('>')
            && rel_close > 0
        {
            i = i + 1 + rel_close + 1;
            continue;
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn match_md_link(input: &str, open_bracket: usize) -> Option<(&str, usize)> {
    let close_bracket = input[open_bracket + 1..].find(']')? + open_bracket + 1;
    if input.as_bytes().get(close_bracket + 1) != Some(&b'(') {
        return None;
    }
    let close_paren = input[close_bracket + 2..].find(')')? + close_bracket + 2;
    Some((&input[open_bracket + 1..close_bracket], close_paren + 1))
}

fn collapse_blank_runs(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut newlines = 0usize;
    for ch in input.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push('\n');
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_policy_check_interval_offer_and_store_rules() {
        const HOUR: i64 = 60 * 60 * 1000;
        const NOW: i64 = 1_700_000_000_000;
        assert_eq!(CHECK_INTERVAL_MS, 12 * HOUR);
        assert!(UpdatePolicy::is_check_due(true, 0, NOW));
        assert!(!UpdatePolicy::is_check_due(
            true,
            NOW - 11 * HOUR - 59 * 60 * 1000,
            NOW
        ));
        assert!(UpdatePolicy::is_check_due(true, NOW - 12 * HOUR, NOW));
        assert!(UpdatePolicy::is_check_due(true, NOW + 5 * HOUR, NOW));
        assert!(!UpdatePolicy::is_check_due(false, 0, NOW));

        let release = Release::parse(r#"{"tag_name":"v0.8.0"}"#).unwrap();
        assert!(UpdatePolicy::should_offer(
            Some(&release),
            "0.7.0",
            Some("")
        ));
        assert!(!UpdatePolicy::should_offer(
            Some(&release),
            "0.7.0",
            Some("v0.8.0")
        ));
        assert!(UpdatePolicy::is_managed_by_store(Some(PLAY_STORE)));
        assert!(UpdatePolicy::is_self_update_available(None, true));
        assert!(!UpdatePolicy::is_self_update_available(
            Some(PLAY_STORE),
            true
        ));
    }

    #[test]
    fn release_notes_strip_markdown_and_avoid_splitting_surrogate_pairs() {
        let notes = "## What's new\r\n\r\n- **Faster** pairing\r\n* See [the docs](https://example.com/docs)\r\n> Note `code`\r\n![shot](https://example.com/a.png)\r\n<br>Done";
        assert_eq!(
            ReleaseNotes::plain(notes, 1000),
            "What's new\n\n• Faster pairing\n• See the docs\nNote code\n\nDone"
        );
        assert_eq!(ReleaseNotes::plain("abcd😀efgh", 6), "abcd…");
    }
}
