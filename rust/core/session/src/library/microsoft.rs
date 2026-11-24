//! Microsoft Store and Xbox PC package manifest parsing and AUMID validation.
use super::stores::{identifier, relative_file};
use super::{Game, Launch, Source};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::path::Path;

/// The part of `MicrosoftGame.config` that names the game and its programs.
#[derive(Deserialize)]
struct MicrosoftConfig {
    #[serde(rename = "Identity")]
    identity: MicrosoftIdentity,
    #[serde(rename = "ShellVisuals")]
    visuals: Option<MicrosoftVisuals>,
    #[serde(rename = "ExecutableList")]
    executables: Option<MicrosoftExecutables>,
}
#[derive(Deserialize)]
struct MicrosoftIdentity {
    #[serde(rename = "@Name")]
    name: String,
}
#[derive(Deserialize)]
struct MicrosoftVisuals {
    #[serde(rename = "@DefaultDisplayName", default)]
    name: String,
    #[serde(rename = "@Square150x150Logo", default)]
    logo: String,
}
#[derive(Deserialize)]
struct MicrosoftExecutables {
    #[serde(rename = "Executable", default)]
    list: Vec<MicrosoftExecutable>,
}
#[derive(Deserialize)]
struct MicrosoftExecutable {
    #[serde(rename = "@Name", default)]
    name: String,
    #[serde(rename = "@Id", default)]
    id: String,
    #[serde(rename = "@TargetDeviceFamily", default)]
    family: String,
}
/// The applications of an `AppxManifest.xml`.
#[derive(Deserialize)]
struct AppxManifest {
    #[serde(rename = "Applications")]
    applications: Option<AppxApplications>,
}
#[derive(Deserialize)]
struct AppxApplications {
    #[serde(rename = "Application", default)]
    list: Vec<AppxApplication>,
}
#[derive(Deserialize)]
struct AppxApplication {
    #[serde(rename = "@Id")]
    id: String,
    #[serde(rename = "@Executable", default)]
    executable: String,
}

/// An installed Microsoft Store or Xbox package, as Windows reports it.
pub struct StorePackage<'a> {
    /// `Name_PublisherId`.
    pub family: &'a str,
    /// The package's own display name; may be empty or a resource reference.
    pub display_name: &'a str,
    /// Its `AppxManifest.xml`.
    pub manifest: &'a str,
}

/// A name worth showing: not empty and not an unresolved `ms-resource:` reference.
fn shown_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty() && !name.to_ascii_lowercase().starts_with("ms-resource:")).then_some(name)
}
fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// An application user model id (`Name_PublisherId!App`), which `shell:AppsFolder\` takes to
/// start the package. Only the characters packages are named with, so nothing in it reaches
/// the shell as anything but one name.
fn aumid(family: &str, app: &str) -> Result<String> {
    let name_chars = |text: &str, extra: &[u8]| {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    let (name, publisher) = family
        .split_once('_')
        .context("The package family name is not Name_PublisherId.")?;
    ensure!(
        name_chars(name, b".-") && name_chars(publisher, b".-"),
        "The package family name {family:?} has characters a package name cannot have."
    );
    ensure!(
        name_chars(app, b"._-"),
        "The application id {app:?} has characters an application id cannot have."
    );
    Ok(format!("{family}!{app}"))
}

/// The application user model id in a `shell:AppsFolder\` launch address, if that is all the
/// address holds. The host checks this again at launch, since `apps.json` can be edited.
pub fn store_launch_target(uri: &str) -> Option<&str> {
    let target = uri.strip_prefix("shell:AppsFolder\\")?;
    let (family, app) = target.split_once('!')?;
    aumid(family, app).ok().map(|_| target)
}

/// A game from the `MicrosoftGame.config` in `folder` (the game's install folder, where its
/// programs are). `package` is the registered package it belongs to; without one the game is
/// listed as unavailable, since nothing can start it. A config with no program for PCs is a
/// console-only title (`None`). Nothing here bypasses a licence or a sign in: the package's
/// own entry point starts, as from the Start menu.
pub fn microsoft(
    config: &str,
    folder: &Path,
    package: Option<&StorePackage>,
) -> Result<Option<Game>> {
    let config: MicrosoftConfig = quick_xml::de::from_str(config.trim_start_matches('\u{feff}'))
        .context("MicrosoftGame.config is not readable.")?;
    let id = identifier(config.identity.name.trim())?.to_owned();
    let programs: Vec<_> = config
        .executables
        .map(|list| list.list)
        .unwrap_or_default()
        .into_iter()
        .filter(|program| program.family.is_empty() || program.family.eq_ignore_ascii_case("PC"))
        .collect();
    // A console-only title, or a config that lists no program.
    let Some(program) = programs.first() else {
        return Ok(None);
    };
    let name = shown_name(config.visuals.as_ref().map_or("", |v| &v.name))
        .or_else(|| package.and_then(|p| shown_name(p.display_name)))
        .unwrap_or(&id)
        .to_owned();
    let apps = match package {
        Some(package) => {
            let manifest: AppxManifest =
                quick_xml::de::from_str(package.manifest.trim_start_matches('\u{feff}'))
                    .context("AppxManifest.xml is not readable.")?;
            let apps = manifest.applications.map(|a| a.list).unwrap_or_default();
            // A package with no application (a DLC or content pack beside its game) starts
            // nothing: no game of its own.
            if apps.is_empty() {
                return Ok(None);
            }
            Some(apps)
        }
        None => None,
    };
    let launch = package
        .zip(apps)
        .map(|(package, apps)| -> Result<Launch> {
            // The config's Id is the manifest's application Id; the program's file name is
            // the second way to find it, and the package's one GameLaunchHelper.exe
            // application the third: an Xbox PC game starts through it, and it starts the
            // program.
            let app = apps
                .iter()
                .find(|app| !program.id.is_empty() && app.id == program.id)
                .or_else(|| {
                    apps.iter().find(|app| {
                        file_name_of(&app.executable)
                            .eq_ignore_ascii_case(file_name_of(&program.name))
                    })
                })
                .or_else(|| {
                    let mut helpers = apps.iter().filter(|app| {
                        file_name_of(&app.executable).eq_ignore_ascii_case("GameLaunchHelper.exe")
                    });
                    helpers.next().filter(|_| helpers.next().is_none())
                })
                .with_context(|| {
                    format!(
                        "None of the package's applications starts {}.",
                        program.name
                    )
                })?;
            Ok(Launch::Uri {
                uri: format!("shell:AppsFolder\\{}", aumid(package.family, &app.id)?),
            })
        })
        .transpose()?;
    let mut game = Game::new(Source::Microsoft, id, name, folder, launch);
    if package.is_none() {
        game.unavailable_reason = Some(
            "This game is not installed for this Windows user. Install or repair it in the Xbox app or the Microsoft Store."
                .into(),
        );
    }
    game.cover = config
        .visuals
        .and_then(|v| relative_file(folder, v.logo.trim()).ok())
        .filter(|path| path.is_file());
    Ok(Some(game))
}
