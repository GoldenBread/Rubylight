//! SemVer parsing, comparison and strict release-tag validation shared by Android and Desktop.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// A release version such as `v0.10.0` or `1.2.3-rc.1`; build metadata after `+` is ignored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemVer {
    pub major: i32,
    pub minor: i32,
    pub patch: i32,
    pub prerelease: Option<String>,
}

impl SemVer {
    /// Parses a version string; a leading `v`/`V` is ignored and missing minor/patch default to 0.
    pub fn parse(text: &str) -> Option<Self> {
        let s = text.trim();
        let s = s
            .strip_prefix('v')
            .or_else(|| s.strip_prefix('V'))
            .unwrap_or(s);
        if s.is_empty() {
            return None;
        }
        // Strip optional +build metadata after validating its character set.
        let (before_build, build) = match s.split_once('+') {
            Some((head, tail)) => (head, Some(tail)),
            None => (s, None),
        };
        if let Some(b) = build
            && (b.is_empty() || !b.bytes().all(is_ident_char))
        {
            return None;
        }
        let (core, pre) = match before_build.split_once('-') {
            Some((head, tail)) => (head, Some(tail)),
            None => (before_build, None),
        };
        if let Some(p) = pre
            && (p.is_empty() || !p.bytes().all(is_ident_char))
        {
            return None;
        }
        let mut parts = core.split('.');
        let major = parse_nonneg_i32(parts.next()?)?;
        let minor = match parts.next() {
            Some(p) => parse_nonneg_i32(p)?,
            None => 0,
        };
        let patch = match parts.next() {
            Some(p) => parse_nonneg_i32(p)?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            prerelease: pre.map(str::to_owned),
        })
    }

    pub fn is_prerelease(&self) -> bool {
        self.prerelease.is_some()
    }

    /// Whether `candidate` names a newer version than `current`. Unparseable versions are never newer.
    pub fn is_newer(candidate: &str, current: &str) -> bool {
        match (Self::parse(candidate), Self::parse(current)) {
            (Some(a), Some(b)) => a > b,
            _ => false,
        }
    }

    /// The normalized `major.minor.patch[-prerelease]` form without a leading `v`.
    pub fn normalize(text: &str) -> String {
        match Self::parse(text) {
            Some(v) => v.to_string(),
            None => text.trim().to_owned(),
        }
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.prerelease {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl PartialEq for SemVer {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for SemVer {}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| match (&self.prerelease, &other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => cmp_prerelease(a, b),
            })
    }
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-'
}

fn parse_nonneg_i32(part: &str) -> Option<i32> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse::<i32>().ok()
}

fn cmp_numeric_str(a: &str, b: &str) -> Ordering {
    let a_trim = a.trim_start_matches('0');
    let b_trim = b.trim_start_matches('0');
    a_trim
        .len()
        .cmp(&b_trim.len())
        .then_with(|| a_trim.cmp(b_trim))
}

fn java_split_dot(s: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split('.').collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
}

fn cmp_prerelease(a: &str, b: &str) -> Ordering {
    let a_parts = java_split_dot(a);
    let b_parts = java_split_dot(b);
    for (sa, sb) in a_parts.iter().zip(b_parts.iter()) {
        let num_a = !sa.is_empty() && sa.bytes().all(|c| c.is_ascii_digit());
        let num_b = !sb.is_empty() && sb.bytes().all(|c| c.is_ascii_digit());
        let ord = match (num_a, num_b) {
            (true, true) => cmp_numeric_str(sa, sb),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => sa.cmp(sb),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a_parts.len().cmp(&b_parts.len())
}

/// Parses a desktop version string (`[v]M.m[.p][-pre][+build]`, 2 or 3 core components).
fn parse_desktop(text: &str) -> Option<([u64; 3], Option<&str>)> {
    let trimmed = text.trim();
    let trimmed = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let (core, pre) = match trimmed.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (trimmed, None),
    };
    let core = core.split('+').next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let numbers = [
        parts.next()??,
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
    ];
    parts
        .next()
        .is_none()
        .then_some((numbers, pre.filter(|p| !p.is_empty())))
}

/// Whether `tag` is a strict stable desktop release tag (`[v]digits.digits[.digits]` with no
/// whitespace, no prerelease, and no `+build` metadata, because `tag` becomes a filename/URL).
pub fn is_strict_stable_tag(tag: &str) -> bool {
    let Some(stripped) = Some(tag.strip_prefix('v').unwrap_or(tag)) else {
        return false;
    };
    !stripped.is_empty()
        && stripped.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && matches!(parse_desktop(tag), Some((_, None)))
}

/// Whether `tag` is a strict stable desktop release newer than `current`.
pub fn newer_stable_tag(tag: &str, current: &str) -> bool {
    if !is_strict_stable_tag(tag) {
        return false;
    }
    let (Some((latest, None)), Some((running, pre))) = (parse_desktop(tag), parse_desktop(current))
    else {
        return false;
    };
    latest > running || (latest == running && pre.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numbers_and_ignores_v_and_build_metadata() {
        assert!(SemVer::is_newer("0.10.0", "0.9.9"));
        assert!(!SemVer::is_newer("0.9.9", "0.10.0"));
        assert!(SemVer::is_newer("1.0.0", "0.99.99"));
        assert!(!SemVer::is_newer("v0.7.0", "0.7.0"));
        assert_eq!(SemVer::parse("1.2"), SemVer::parse("v1.2.0"));
        assert_eq!(SemVer::normalize("V0.7.1"), "0.7.1");
        assert_eq!(SemVer::parse("1.2.3+abc"), SemVer::parse("1.2.3+xyz"));
        assert!(SemVer::parse("nightly").is_none());
        assert!(SemVer::parse("").is_none());
        assert!(SemVer::parse(" v1.0.0 ").is_some());
        assert_eq!(SemVer::normalize("nightly"), "nightly");
    }

    #[test]
    fn prerelease_ordering_and_big_numeric_segments_match_semver() {
        assert!(SemVer::is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!SemVer::is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(SemVer::is_newer("1.0.0-rc.10", "1.0.0-rc.2"));
        assert!(SemVer::is_newer("1.0.0-rc.1", "1.0.0-beta.5"));
        assert!(SemVer::is_newer("1.0.0-rc.1.1", "1.0.0-rc.1"));
        // Huge numeric prerelease identifiers beyond u64 still order numerically.
        assert!(SemVer::is_newer(
            "1.0.0-99999999999999999999999999",
            "1.0.0-10000000000000000000000000"
        ));
        assert!(SemVer::is_newer("1.0.0-rc.002", "1.0.0-rc.1"));
    }

    #[test]
    fn strict_stable_tags_match_desktop_rules() {
        assert!(newer_stable_tag("v4.0.1", "4.0.0"));
        assert!(newer_stable_tag("v4.1.0", "4.0.9"));
        assert!(newer_stable_tag("5.0.0", "4.9.9"));
        assert!(newer_stable_tag("v4.0.0", "4.0.0-rc.3"));
        assert!(newer_stable_tag("v4.1", "4.0.0"));
        assert!(!newer_stable_tag("v4.0.0", "4.0.0"));
        assert!(!newer_stable_tag("v3.9.9", "4.0.0"));
        assert!(!newer_stable_tag("v4.1.0-rc.1", "4.0.0"));
        assert!(!newer_stable_tag("nightly", "4.0.0"));
        assert!(!newer_stable_tag("v4.1.0", "a local build"));
        assert!(!newer_stable_tag("v4.1.0.2", "4.0.0"));
        for odd in ["v9.9.9+a/b", "9.9.9+build", "v9.9.9/..", " v9.9.9"] {
            assert!(!newer_stable_tag(odd, "4.0.0"), "{odd}");
        }
        assert!(newer_stable_tag("v4.1.0", "4.0.0+local"));
    }
}
