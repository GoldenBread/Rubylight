//! Webhooks: the host POSTs JSON to the URLs of the `webhooks` setting when
//! something happens (the host's `webhooks` module sends them; `docs/webhooks.md`
//! is the receiver's guide).
//!
//! Each delivery is [`Delivery`] as JSON with the headers [`HEADER_EVENT`],
//! [`HEADER_DELIVERY`], [`HEADER_TIMESTAMP`] and [`HEADER_SIGNATURE`], whose
//! value is [`SIGNATURE_PREFIX`] and [`sign`] of the body with the hook's
//! secret, so a receiver can check the host sent it. The signed body carries
//! the delivery id and time, so a receiver refuses replays by both:
//! [`REPLAY_WINDOW_MS`].
use crate::config_ext::ConfigExt;
use anyhow::{Context, Result, bail};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The events a hook may subscribe to.
pub const EVENTS: &[&str] = &[
    "pairing.paired",
    "pairing.unpaired",
    "app.started",
    "app.stopped",
    "session.started",
    "session.ended",
    "power.restart",
    "power.shutdown",
    "update.started",
];
pub const HEADER_EVENT: &str = "X-Rubylight-Event";
/// A UUID per delivery, the same as [`Delivery::delivery`].
pub const HEADER_DELIVERY: &str = "X-Rubylight-Delivery";
pub const HEADER_SIGNATURE: &str = "X-Rubylight-Signature";
/// [`Delivery::time_unix_ms`] again, in decimal. The signature covers the
/// body's copy, which is the one to check.
pub const HEADER_TIMESTAMP: &str = "X-Rubylight-Timestamp";
/// The event of the console's "Send test", sent to one hook whatever it
/// subscribes to. Not one of [`EVENTS`].
pub const TEST_EVENT: &str = "webhook.test";
/// How far a delivery's time may be from the receiver's clock before it
/// refuses it as a replay; retries resend the same body well within it.
pub const REPLAY_WINDOW_MS: u64 = 5 * 60 * 1000;
/// Before the hex digest in [`HEADER_SIGNATURE`].
pub const SIGNATURE_PREFIX: &str = "sha256=";

/// One entry of the `webhooks` setting.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Webhook {
    /// An `http://` or `https://` URL.
    pub url: String,
    /// Some of [`EVENTS`].
    pub events: Vec<String>,
    /// The HMAC key; never logged. Stored separately in [`secrets_path`] once migrated.
    #[serde(default)]
    pub secret: String,
}

impl std::fmt::Debug for Webhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Webhook")
            .field("url", &self.url)
            .field("events", &self.events)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// The body of one delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Delivery<'a> {
    pub event: &'a str,
    pub delivery: &'a str,
    pub time_unix_ms: u64,
    pub host: DeliveryHost<'a>,
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeliveryHost<'a> {
    pub name: &'a str,
    pub unique_id: &'a str,
}

/// `webhooks` ([`parse_webhooks`]). A list it refuses is reported without its
/// text, which holds the secrets, and no hook is used.
pub const SECRETS_FILE: &str = "credentials/webhook_secrets.json";

/// Where webhook secrets are stored outside `sunshine.conf`, in a folder
/// restricted to Administrators and SYSTEM.
pub fn secrets_path(directory: &Path) -> PathBuf {
    directory.join("credentials").join("webhook_secrets.json")
}

#[derive(Default, Deserialize, Serialize)]
struct SecretStore {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// Loads stored webhook secrets keyed by webhook URL. Returns empty when no
/// secret file exists yet.
pub fn load_secrets(path: &Path) -> Result<BTreeMap<String, String>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let store: SecretStore =
                serde_json::from_slice(&bytes).context("reading stored webhook secrets")?;
            Ok(store.secrets)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Serializes webhook secrets keyed by URL in the [`SecretStore`] JSON format.
pub fn encode_secrets(secrets: &BTreeMap<String, String>) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&SecretStore {
        version: 1,
        secrets: secrets.clone(),
    })?)
}

/// Persists webhook secrets keyed by URL atomically inside the protected
/// credentials folder.
pub fn save_secrets(path: &Path, secrets: &BTreeMap<String, String>) -> Result<()> {
    crate::state::atomic_write(path, &encode_secrets(secrets)?)
}

/// Reads hooks from `config` merged with secrets from `secrets_path(directory)`.
/// Inline secrets in `config` still work as a fallback until migrated.
pub fn load_from_profile(
    config: &butterpollo_config::config::Config,
    directory: &Path,
) -> Vec<Webhook> {
    let text = config.get("webhooks", "[]");
    let mut hooks = match parse_webhooks_with(text, false) {
        Ok(hooks) => hooks,
        Err(error) => {
            let error = error.to_string();
            if butterpollo_config::config::first_report("webhooks", &error) {
                tracing::warn!(
                    key = "webhooks",
                    error,
                    "unsupported setting value; using the default"
                );
            }
            return Vec::new();
        }
    };
    let secrets = match load_secrets(&secrets_path(directory)) {
        Ok(secrets) => secrets,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "webhook secrets unavailable");
            BTreeMap::new()
        }
    };
    hooks.retain_mut(|hook| {
        if let Some(secret) = secrets.get(&hook.url).filter(|s| !s.is_empty()) {
            hook.secret = secret.clone();
        }
        if hook.secret.is_empty() {
            let origin = origin(&hook.url);
            if butterpollo_config::config::first_report("webhooks", &origin) {
                tracing::warn!(
                    key = "webhooks",
                    origin,
                    "webhook has no secret; skipping delivery"
                );
            }
            false
        } else {
            true
        }
    });
    hooks
}

/// Splits validated hooks into a secret-free JSON list for `sunshine.conf` and a
/// `url -> secret` map for [`secrets_path`].
pub fn split_for_storage(hooks: &[Webhook]) -> Result<(String, BTreeMap<String, String>)> {
    let mut config_hooks = Vec::with_capacity(hooks.len());
    let mut secrets = BTreeMap::new();
    for hook in hooks {
        if let Some(secret) = (!hook.secret.is_empty()).then_some(&hook.secret) {
            secrets.insert(hook.url.clone(), secret.clone());
        }
        config_hooks.push(json!({
            "url": hook.url,
            "events": hook.events,
        }));
    }
    Ok((Value::Array(config_hooks).to_string(), secrets))
}

/// Moves inline webhook secrets out of `config` into [`secrets_path`], rewriting
/// `config`'s `webhooks` value without secrets. Returns whether `config` changed.
pub fn migrate_from_config(
    config: &mut butterpollo_config::config::Config,
    directory: &Path,
) -> Result<bool> {
    let Some(raw) = config.values.get("webhooks") else {
        return Ok(false);
    };
    let Ok(mut hooks) = parse_webhooks_with(raw, false) else {
        return Ok(false);
    };
    let mut had_inline_secret = false;
    let mut secrets = load_secrets(&secrets_path(directory))?;
    for hook in &mut hooks {
        if !hook.secret.is_empty() {
            had_inline_secret = true;
            secrets
                .entry(hook.url.clone())
                .or_insert_with(|| hook.secret.clone());
        }
    }
    let has_secret_field = serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .is_some_and(|items| items.iter().any(|item| item.get("secret").is_some()));
    if !had_inline_secret && !has_secret_field {
        return Ok(false);
    }
    if had_inline_secret {
        save_secrets(&secrets_path(directory), &secrets)?;
    }
    let (stripped, _) = split_for_storage(&hooks)?;
    let update = serde_json::Map::from_iter([("webhooks".into(), Value::String(stripped))]);
    config.update(&update)?;
    Ok(true)
}

/// Strips `"secret"` fields from a JSON `webhooks` array (including legacy/test entries with arbitrary
/// events) and returns `(stripped_json, extracted_url_to_secret)`.
pub fn split_json_secrets(raw: &str) -> Option<(String, BTreeMap<String, String>)> {
    let mut list = serde_json::from_str::<Value>(raw).ok()?;
    let items = list.as_array_mut()?;
    let mut secrets = BTreeMap::new();
    for item in items.iter_mut() {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        let url = object
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if let Some(secret) = object
            .remove("secret")
            .and_then(|value| value.as_str().map(str::to_owned))
            && !url.is_empty()
            && !secret.is_empty()
        {
            secrets.insert(url, secret);
        }
    }
    Some((list.to_string(), secrets))
}

/// The `webhooks` setting: validated hooks, or empty on malformed input. Errors report only the
/// first failure and never log secrets.
pub fn from_config(config: &butterpollo_config::config::Config) -> Vec<Webhook> {
    parse_webhooks(config.get("webhooks", "[]")).unwrap_or_else(|error| {
        // The outermost message only: it names a hook by its origin, never its secret.
        let error = error.to_string();
        if butterpollo_config::config::first_report("webhooks", &error) {
            tracing::warn!(
                key = "webhooks",
                error,
                "unsupported setting value; using the default"
            );
        }
        Vec::new()
    })
}

/// Reads the `webhooks` setting: a JSON list of [`Webhook`]s. An unknown
/// event, a URL that is not http(s), an empty event list or an empty secret
/// refuses the whole list. Its errors name a hook by [`origin`] alone: they
/// reach the log, and a URL's path or query may hold a token.
pub fn parse_webhooks(text: &str) -> Result<Vec<Webhook>> {
    parse_webhooks_with(text, true)
}

/// Like [`parse_webhooks`], but permits empty inline secrets when stored in [`secrets_path`].
pub fn parse_webhooks_allow_empty_secrets(text: &str) -> Result<Vec<Webhook>> {
    parse_webhooks_with(text, false)
}

fn parse_webhooks_with(text: &str, require_secret: bool) -> Result<Vec<Webhook>> {
    let hooks: Vec<Webhook> =
        serde_json::from_str(text).context("webhooks is not a JSON list of hooks")?;
    for (at, hook) in hooks.iter().enumerate() {
        let rest = hook
            .url
            .strip_prefix("https://")
            .or_else(|| hook.url.strip_prefix("http://"));
        if rest.is_none_or(|rest| rest.is_empty() || rest.chars().any(char::is_whitespace)) {
            bail!("the URL of webhook {} is not an http(s) URL", at + 1);
        }
        let origin = origin(&hook.url);
        if hook.events.is_empty() {
            bail!("webhook {origin} subscribes to no events");
        }
        if let Some(event) = hook.events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
            bail!("webhook {origin} names an unknown event {event:?}");
        }
        if require_secret && hook.secret.is_empty() {
            bail!("webhook {origin} has no secret");
        }
    }
    Ok(hooks)
}

/// `scheme://host:port` of an http(s) `url`, without the user name, password,
/// path, query or fragment, any of which may hold a token.
pub fn origin(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    format!("{scheme}://{host}")
}

/// Lowercase hex HMAC-SHA256 of `body` with `secret`.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hex::encode(hmac::sign(&key, body).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_matches_rfc_4231_case_2() {
        assert_eq!(
            sign("Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn a_valid_list_parses_and_hides_its_secret() {
        let hooks = parse_webhooks(
            r#"[{"url":"https://example.test/hook","events":["app.started","session.ended"],"secret":"s3cret"},
                {"url":"http://192.168.1.5:8080/","events":["pairing.paired"],"secret":"x"}]"#,
        )
        .unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].events, ["app.started", "session.ended"]);
        assert!(!format!("{hooks:?}").contains("s3cret"));
        assert!(parse_webhooks("[]").unwrap().is_empty());
    }

    #[test]
    fn a_bad_hook_refuses_the_list() {
        for bad in [
            r#"[{"url":"https://a.test","events":["app.crashed"],"secret":"s"}]"#,
            r#"[{"url":"ftp://a.test","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"https://","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"https://a test","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"a.test","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"https://a.test","events":[],"secret":"s"}]"#,
            r#"[{"url":"https://a.test","events":["app.started"],"secret":""}]"#,
            r#"[{"url":"https://a.test","events":["app.started"]}]"#,
            r#"[{"url":"https://a.test","events":["app.started"],"secret":"s","extra":1}]"#,
            r#"{"url":"https://a.test"}"#,
            "not json",
        ] {
            assert!(parse_webhooks(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_refused_hook_is_named_without_its_token() {
        // Discord and Slack hooks hold their token in the path; the error reaches the log.
        let token = "T0K3N";
        for bad in [
            r#"[{"url":"https://discord.test/api/webhooks/1/T0K3N","events":[],"secret":"s"}]"#,
            r#"[{"url":"https://hooks.test/services/T0K3N?x=1","events":["app.crashed"],"secret":"s"}]"#,
            r#"[{"url":"https://user:T0K3N@hooks.test/a","events":["app.started"],"secret":""}]"#,
        ] {
            let error = format!("{:#}", parse_webhooks(bad).unwrap_err());
            assert!(!error.contains(token), "{error}");
            assert!(error.contains(".test"), "{error}");
        }
        let error = parse_webhooks(
            r#"[{"url":"https://a.test","events":["app.started"],"secret":"s"},
                {"url":"ftp://T0K3N@b.test/T0K3N","events":["app.started"],"secret":"s"}]"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            !error.contains(token) && error.contains("webhook 2"),
            "{error}"
        );
        assert_eq!(
            origin("https://user:pw@example.com:8443/a/b?token=x#f"),
            "https://example.com:8443"
        );
        assert_eq!(origin("http://192.168.1.5\\x"), "http://192.168.1.5");
    }

    #[test]
    fn a_delivery_has_the_documented_shape() {
        let body = Delivery {
            event: "app.started",
            delivery: "0b6f2d8a-4c11-4c52-9e10-5e0f4b1c7a3d",
            time_unix_ms: 1_792_000_000_000,
            host: DeliveryHost {
                name: "GAMING-PC",
                unique_id: "5E0F4B1C",
            },
            data: serde_json::json!({"app": "Desktop"}),
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({
                "event": "app.started",
                "delivery": "0b6f2d8a-4c11-4c52-9e10-5e0f4b1c7a3d",
                "time_unix_ms": 1_792_000_000_000u64,
                "host": {"name": "GAMING-PC", "unique_id": "5E0F4B1C"},
                "data": {"app": "Desktop"},
            })
        );
    }

    #[test]
    fn config_webhooks_fall_back_to_empty_on_invalid_json_or_url() {
        use butterpollo_config::config::Config;
        assert!(from_config(&Config::default()).is_empty());
        for bad in [
            "webhooks = [{\"url\": \"ftp://x\", \"events\": [\"app.started\"], \"secret\": \"s\"}]",
            "webhooks = not json",
        ] {
            assert!(
                from_config(&Config::parse(bad).unwrap()).is_empty(),
                "{bad}"
            );
        }
        let valid = Config::parse(
            "webhooks = [{\"url\": \"https://example.test/hook\", \"events\": [\"app.started\"], \"secret\": \"s\"}]",
        )
        .unwrap();
        assert_eq!(from_config(&valid).len(), 1);
    }

    #[test]
    fn secrets_store_round_trips_and_migrates_out_of_sunshine_conf() {
        use butterpollo_config::config::Config;

        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::parse(
            "webhooks = [{\"url\": \"https://example.test/hook\", \"events\": [\"app.started\"], \"secret\": \"top-secret\"}]\n",
        )
        .unwrap();
        assert!(migrate_from_config(&mut config, dir.path()).unwrap());
        assert!(!config.text().contains("top-secret"));
        // Re-running migration is a no-op once inline secrets are removed.
        assert!(!migrate_from_config(&mut config, dir.path()).unwrap());

        let loaded = load_from_profile(&config, dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].secret, "top-secret");

        // Hooks without a secret in either sunshine.conf or webhook_secrets.json are skipped.
        let missing_cfg = Config::parse(
            "webhooks = [{\"url\": \"https://other.test/hook\", \"events\": [\"app.started\"]}]\n",
        )
        .unwrap();
        assert!(load_from_profile(&missing_cfg, dir.path()).is_empty());
    }
}
