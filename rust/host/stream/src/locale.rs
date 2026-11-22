//! Locale dictionaries bundled from `rust/assets/locale/*.json`, shared by `couch_menu`
//! and the web console's HTML localizer.

use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};

macro_rules! catalogues {
    ($($locale:literal),* $(,)?) => {
        pub const CATALOGUES: &[(&str, &str)] = &[$((
            $locale,
            include_str!(concat!("../../../assets/locale/", $locale, ".json"))
        )),*];
    };
}
catalogues!(
    "bg", "cs", "de", "en", "en_GB", "en_US", "es", "fr", "hu", "it", "ja", "ko", "pl", "pt",
    "pt_BR", "ru", "sv", "tr", "uk", "vi", "zh", "zh_TW"
);

pub type Dictionary = BTreeMap<String, String>;
type Cache = Mutex<BTreeMap<&'static str, Arc<Dictionary>>>;

pub fn language_name(code: &str) -> &str {
    match code {
        "en" => "English",
        "en_GB" => "English (UK)",
        "en_US" => "English (US)",
        "bg" => "Български",
        "cs" => "Čeština",
        "de" => "Deutsch",
        "es" => "Español",
        "fr" => "Français",
        "hu" => "Magyar",
        "it" => "Italiano",
        "ja" => "日本語",
        "ko" => "한국어",
        "pl" => "Polski",
        "pt" => "Português",
        "pt_BR" => "Português (Brasil)",
        "ru" => "Русский",
        "sv" => "Svenska",
        "tr" => "Türkçe",
        "uk" => "Українська",
        "vi" => "Tiếng Việt",
        "zh" => "中文（简体）",
        "zh_TW" => "中文（繁體）",
        _ => code,
    }
}

pub fn locale(raw: &str) -> &'static str {
    let normalized = raw.trim().replace('-', "_");
    CATALOGUES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&normalized))
        .or_else(|| {
            CATALOGUES.iter().find(|(name, _)| {
                name.eq_ignore_ascii_case(normalized.split('_').next().unwrap_or(""))
            })
        })
        .map_or("en", |(name, _)| *name)
}

pub fn dictionary(language: &'static str) -> Arc<Dictionary> {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache
        .entry(language)
        .or_insert_with(|| {
            fn flatten(value: &Value, path: &str, output: &mut Dictionary) {
                if let Some(object) = value.as_object() {
                    for (key, value) in object {
                        flatten(value, &format!("{path}/{key}"), output);
                    }
                } else if let Some(text) = value.as_str() {
                    output.insert(path.into(), text.into());
                }
            }
            let mut result = Dictionary::new();
            let source = CATALOGUES
                .iter()
                .find(|(name, _)| *name == language)
                .unwrap()
                .1;
            flatten(
                &serde_json::from_str(source).expect("bundled locale JSON"),
                "",
                &mut result,
            );
            Arc::new(result)
        })
        .clone()
}

fn key(english: &str) -> Option<&'static str> {
    match english {
        "Overview" => Some("/navbar/home"),
        "Library" => Some("/navbar/applications"),
        "Devices" => Some("/clients/nav"),
        "Settings" => Some("/navbar/configuration"),
        "API tokens" => Some("/navbar/api_tokens"),
        "Sign out" => Some("/navbar/logout"),
        "Sign in" => Some("/auth/login_sign_in"),
        "System" => Some("/navbar/theme_auto"),
        "Appearance" => Some("/navbar/toggle_theme"),
        "Application name" => Some("/apps/app_name"),
        "Command (leave empty for desktop)" => Some("/apps/cmd"),
        "Working directory" => Some("/apps/working_dir"),
        "Use default" => Some("/_common/default_global"),
        _ => None,
    }
}

fn english_keys() -> &'static BTreeMap<String, String> {
    static KEYS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys = BTreeMap::new();
        for (path, text) in dictionary("en").iter() {
            if !text.contains(['<', '>', '{', '}']) && !path.starts_with("/couch_menu/") {
                keys.entry(text.to_ascii_lowercase())
                    .or_insert_with(|| path.clone());
            }
        }
        keys
    })
}

pub fn lookup<'a>(language: &'a Dictionary, english: &str, path: Option<&str>) -> Option<&'a str> {
    let path = path.or_else(|| key(english)).or_else(|| {
        english_keys()
            .get(&english.to_ascii_lowercase())
            .map(String::as_str)
    })?;
    language
        .get(path)
        .filter(|v| !v.is_empty() && !v.contains(['<', '>', '{', '}']))
        .map(String::as_str)
}

/// A catalogue string by its path (`couch_menu/title`): German for a `de*`
/// locale and English for every other one, the two the couch menu has.
pub fn catalogue(raw_locale: &str, path: &str) -> Option<String> {
    let language = if locale(raw_locale) == "de" {
        "de"
    } else {
        "en"
    };
    dictionary(language)
        .get(&format!("/{path}"))
        .filter(|text| !text.is_empty())
        .cloned()
}
