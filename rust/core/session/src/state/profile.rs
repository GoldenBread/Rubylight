//! Profile file paths, web console credentials, and app/preparation command records.
use super::{
    client::{compatible_bool, null_as_default},
    load_json,
};
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use butterpollo_crypto::crypto;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Where a profile keeps its files, as its sunshine.conf names them.
pub struct ProfileFiles {
    pub paired: PathBuf,
    pub credentials: PathBuf,
    pub apps: PathBuf,
    pub aliases: PathBuf,
    pub certificate: PathBuf,
    pub key: PathBuf,
}
impl ProfileFiles {
    pub fn new(config: &Config, directory: &Path) -> Self {
        let credentials = if directory.join("sunshine_credentials.json").exists() {
            "sunshine_credentials.json"
        } else {
            "sunshine_state.json"
        };
        Self {
            paired: config.path("file_state", directory, "sunshine_state.json"),
            credentials: config.path("credentials_file", directory, credentials),
            apps: config.path("file_apps", directory, "apps.json"),
            aliases: config.path("vibeshine_file_state", directory, "vibeshine_state.json"),
            certificate: config.path("cert", directory, "credentials/cacert.pem"),
            key: config.path("pkey", directory, "credentials/cakey.pem"),
        }
    }
}
const PBKDF2_PREFIX: &str = "pbkdf2-sha256";
const PBKDF2_ITERATIONS: u32 = 600_000;
const MAX_PBKDF2_ITERATIONS: u32 = 5_000_000;

fn derive_pbkdf2(salt_hex: &str, password: &str) -> Result<String> {
    let salt = hex::decode(salt_hex).context("invalid credential salt")?;
    let iterations =
        std::num::NonZeroU32::new(PBKDF2_ITERATIONS).context("invalid PBKDF2 iteration count")?;
    let mut derived = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password.as_bytes(),
        &mut derived,
    );
    Ok(format!(
        "{PBKDF2_PREFIX}${PBKDF2_ITERATIONS}${}",
        hex::encode(derived)
    ))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub salt: String,
}
impl Credentials {
    /// The web console's credentials in `path`; None until they are set.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let document = load_json(path, json!({}))?;
        if document.get("username").is_none() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_value(document).context("invalid web credentials")?,
        ))
    }
    pub fn new(username: String, password: &str) -> Result<Self> {
        let username = username.trim().to_owned();
        if username.is_empty() || username.len() > 128 || password.len() < 8 {
            bail!("username required; password must have at least 8 characters");
        }
        let salt = hex::encode(crypto::random::<16>());
        let password = derive_pbkdf2(&salt, password)?;
        Ok(Self {
            username,
            password,
            salt,
        })
    }
    pub fn verifies(&self, username: &str, password: &str) -> bool {
        if !self.username.trim().eq_ignore_ascii_case(username.trim()) {
            return false;
        }
        if let Some(rest) = self.password.strip_prefix(&format!("{PBKDF2_PREFIX}$")) {
            let Some((iters, digest_hex)) = rest.split_once('$') else {
                return false;
            };
            let Ok(iters) = iters.parse::<u32>() else {
                return false;
            };
            let Some(iterations) =
                std::num::NonZeroU32::new(iters).filter(|n| n.get() <= MAX_PBKDF2_ITERATIONS)
            else {
                return false;
            };
            let (Ok(salt), Ok(expected)) = (hex::decode(&self.salt), hex::decode(digest_hex))
            else {
                return false;
            };
            if expected.len() != 32 {
                return false;
            }
            return ring::pbkdf2::verify(
                ring::pbkdf2::PBKDF2_HMAC_SHA256,
                iterations,
                &salt,
                password.as_bytes(),
                &expected,
            )
            .is_ok();
        }
        crypto::matches_hash(
            format!("{password}{}", self.salt).as_bytes(),
            &self.password,
        )
    }

    pub fn needs_rehash(&self) -> bool {
        self.username != self.username.trim()
            || !self
                .password
                .starts_with(&format!("{PBKDF2_PREFIX}${PBKDF2_ITERATIONS}$"))
    }

    pub fn upgrade(&mut self, password: &str) -> Result<()> {
        let upgraded = Self::new(self.username.clone(), password)?;
        *self = upgraded;
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct App {
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub cmd: String,
    #[serde(default, rename = "working-dir", deserialize_with = "null_as_default")]
    pub working_dir: String,
    #[serde(default, rename = "prep-cmd", deserialize_with = "null_as_default")]
    pub prep: Vec<PrepCommand>,
    #[serde(skip)]
    pub computed_id: Option<u32>,
    #[serde(skip)]
    pub aliases: Vec<u32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PrepCommand {
    #[serde(default, deserialize_with = "null_as_default")]
    pub r#do: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub undo: String,
    #[serde(default, deserialize_with = "compatible_bool")]
    pub elevated: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl App {
    pub fn id(&self) -> u32 {
        if let Some(id) = self.computed_id {
            return id;
        }
        if let Some(n) = self.extra.get("id").and_then(Value::as_u64) {
            return n as u32;
        }
        (crc32fast::hash(self.name.as_bytes()) as i32).unsigned_abs()
    }
    pub fn desktop() -> Self {
        Self {
            name: "Desktop".into(),
            cmd: String::new(),
            working_dir: String::new(),
            prep: vec![],
            computed_id: None,
            aliases: vec![],
            extra: BTreeMap::new(),
        }
    }
}
