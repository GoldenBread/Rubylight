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
        if username.is_empty() || username.len() > 128 || password.len() < 8 {
            bail!("username required; password must have at least 8 characters");
        }
        let salt = hex::encode(crypto::random::<8>());
        Ok(Self {
            username,
            password: crypto::legacy_hash(format!("{password}{salt}").as_bytes()),
            salt,
        })
    }
    pub fn verifies(&self, username: &str, password: &str) -> bool {
        self.username.eq_ignore_ascii_case(username)
            && crypto::matches_hash(
                format!("{password}{}", self.salt).as_bytes(),
                &self.password,
            )
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
