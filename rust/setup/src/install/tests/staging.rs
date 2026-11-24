//! Tests for installation targets, service moves, legacy migration and package staging.
use super::super::*;

#[test]
fn a_damaged_installation_is_repaired_by_the_full_installer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let installed = temp.path().join("Butterpollo");
    std::fs::create_dir(&installed)?;
    let service = |folder: &Path| detect::Found {
        service_install: Some(folder.to_owned()),
        ..Default::default()
    };
    assert_eq!(
        target(&detect::Found::default(), &installed)?,
        Target::Fresh
    );
    // A deleted folder or a missing or broken manifest leaves nothing to
    // roll back to, so these do not block a reinstall.
    let deleted = temp.path().join("deleted");
    assert_eq!(target(&service(&deleted), &deleted)?, Target::Fresh);
    assert_eq!(target(&service(&installed), &installed)?, Target::Fresh);
    std::fs::write(installed.join("manifest.json"), "{")?;
    assert_eq!(target(&service(&installed), &installed)?, Target::Fresh);
    std::fs::write(installed.join("manifest.json"), "[]")?;
    assert_eq!(target(&service(&installed), &installed)?, Target::InPlace);
    // Another folder would leave the service without its backup.
    let other = temp.path().join("other");
    assert!(target(&service(&installed), &other).is_err());
    std::fs::create_dir(&other)?;
    assert!(target(&service(&installed), &other).is_err());
    Ok(())
}
#[test]
fn an_installation_that_cannot_stay_moves_to_another_folder() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let installed = temp.path().join("Games/Rubylight");
    std::fs::create_dir_all(&installed)?;
    std::fs::write(installed.join("manifest.json"), "[]")?;
    let found = detect::Found {
        service_install: Some(installed.clone()),
        relocate: Some("Games must be owned by Administrators".into()),
        ..Default::default()
    };
    let program_files = temp.path().join("Program Files/Rubylight");
    assert_eq!(
        target(&found, &program_files)?,
        Target::Move(installed.clone())
    );
    std::fs::create_dir_all(&program_files)?;
    assert_eq!(
        target(&found, &program_files)?,
        Target::Move(installed.clone())
    );
    // Kept in its folder it is updated there, where prepare_install
    // refuses it.
    assert_eq!(target(&found, &installed)?, Target::InPlace);
    Ok(())
}
#[test]
fn a_failed_move_points_the_service_back_at_the_previous_folder() {
    let steps = std::cell::RefCell::new(Vec::new());
    let step = |name: &'static str, ok: bool| {
        let steps = &steps;
        move || {
            steps.borrow_mut().push(name);
            if ok { Ok(()) } else { bail!("{name} failed") }
        }
    };
    assert!(switch_service(step("new", true), step("start", true), step("old", true)).is_ok());
    assert_eq!(steps.take(), ["new", "start"]);
    // The service does not start from the new folder: back to the old
    // one, after it was started there.
    let failure =
        switch_service(step("new", true), step("start", false), step("old", true)).unwrap_err();
    assert_eq!(steps.take(), ["new", "start", "old"]);
    assert!(failure.error.to_string().contains("start failed"));
    assert!(failure.back.is_ok());
    let failure =
        switch_service(step("new", true), step("start", false), step("old", false)).unwrap_err();
    assert_eq!(steps.take(), ["new", "start", "old"]);
    assert!(failure.back.is_err());
    // Not pointed at the new folder: nothing to undo or start there.
    let failure =
        switch_service(step("new", false), step("start", true), step("old", true)).unwrap_err();
    assert_eq!(steps.take(), ["new"]);
    assert!(failure.error.to_string().contains("new failed"));
    assert!(failure.back.is_ok());
}
/// Makes the user running the tests own `path`, with full access for
/// everyone, as a folder that user created and earlier setups locked
/// down but left to that user.
fn give_to_user(path: &Path) -> Result<()> {
    let (_, identity) = system::run(
        &system::system32("whoami.exe"),
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(10),
    )?;
    let sid = identity
        .trim()
        .split(',')
        .nth(1)
        .context("whoami omitted SID")?
        .trim_matches('"');
    give_to(path, sid)
}
/// As [`give_to_user`], for the Users group: not an administrator, so
/// not trusted with a folder above the installation either.
fn give_to_users(path: &Path) -> Result<()> {
    give_to(path, "S-1-5-32-545")
}
fn give_to(path: &Path, sid: &str) -> Result<()> {
    let owner = format!("*{sid}");
    for arguments in [["/setowner", owner.as_str()], ["/grant:r", "*S-1-1-0:(F)"]] {
        let (code, output) = system::run(
            &system::system32("icacls.exe"),
            &[
                &path.display().to_string(),
                arguments[0],
                arguments[1],
                "/Q",
            ],
            Duration::from_secs(10),
        )?;
        assert_eq!(code, 0, "{output}");
    }
    Ok(())
}
#[test]
fn updates_repair_a_user_owned_install_folder_only_under_trusted_parents() -> Result<()> {
    // Taking ownership for SYSTEM and Administrators needs an administrator.
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    // D:\Rubylight, created by the user and locked down by 2.2.1.
    let install = temp.path().join("Rubylight");
    std::fs::create_dir_all(install.join("assets"))?;
    let host = install.join("butterpollo-service.exe");
    std::fs::write(&host, b"service")?;
    give_to_user(&install)?;
    give_to_user(&host)?;
    let error = system::trusted_ancestors(&install).unwrap_err();
    assert!(error.to_string().contains("must be owned by"), "{error:#}");
    assert!(unrepairable(&install).is_none());
    prepare_install(&install)?;
    system::trusted_ancestors(&install)?;
    system::trusted_folder(&host)?;
    assert_eq!(std::fs::read(&host)?, b"service");
    for path in [&install, &host] {
        let (_, output) = system::run(
            &system::system32("icacls.exe"),
            &[&path.display().to_string(), "/findsid", "*S-1-1-0", "/Q"],
            Duration::from_secs(10),
        )?;
        assert!(!output.contains(&path.display().to_string()), "{output}");
    }
    // D:\Games\Rubylight: whoever owns D:\Games can put another folder
    // in its place, so nothing is changed and the error says to move.
    let games = temp.path().join("Games");
    let nested = games.join("Rubylight");
    std::fs::create_dir_all(&nested)?;
    give_to_users(&games)?;
    give_to_user(&nested)?;
    assert!(unrepairable(&nested).is_some());
    let error = prepare_install(&nested).unwrap_err().to_string();
    assert!(
        error.contains(&format!("cannot be updated in {}", nested.display())),
        "{error}"
    );
    assert!(error.contains("must be owned by"), "{error}");
    assert!(
        error.contains(&format!(
            "moves Rubylight to {}",
            detect::default_folder().display()
        )),
        "{error}"
    );
    assert!(system::trusted_folder(&nested).is_err());
    // Nor is a folder with a link inside repaired.
    let linked = temp.path().join("Linked");
    std::fs::create_dir(&linked)?;
    give_to_user(&linked)?;
    std::os::windows::fs::symlink_dir(&games, linked.join("assets"))?;
    assert!(unrepairable(&linked).is_some());
    assert!(prepare_install(&linked).is_err());
    assert!(system::trusted_folder(&games).is_err());
    Ok(())
}
#[test]
fn a_previous_host_is_removed_only_after_its_profile_can_be_imported() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let old = temp.path().join("Vibepollo");
    let profile = temp.path().join("Butterpollo/config");
    let install = temp.path().join("programs/Butterpollo");
    std::fs::create_dir_all(old.join("config"))?;
    std::fs::write(
        old.join("config/sunshine.conf"),
        "encoder=amdvce_experimental",
    )?;
    let product = detect::Product {
        key: "Vibepollo".into(),
        name: "Vibepollo".into(),
        version: "2.0.0".into(),
        location: Some(old.clone()),
        uninstall: None,
        quiet_uninstall: None,
        msi: false,
        root: HKEY_LOCAL_MACHINE,
    };
    let mut found = detect::Found {
        legacy: vec![product.clone()],
        ..Default::default()
    };
    let source = system::win32_path(&old.canonicalize()?)?;
    assert_eq!(migration_source(&found, &profile, &install)?, Some(source));
    assert!(migration_source(&found, &profile, &old).is_err());
    assert!(migration_source(&found, &profile, &old.join("nested")).is_err());
    std::fs::create_dir_all(&profile)?;
    std::fs::write(profile.join("sunshine_state.json"), "existing pairings")?;
    assert!(migration_source(&found, &profile, &install).is_err());
    assert_eq!(
        std::fs::read(profile.join("sunshine_state.json"))?,
        b"existing pairings"
    );
    std::fs::remove_file(profile.join("sunshine_state.json"))?;
    let other = temp.path().join("Sunshine");
    std::fs::create_dir(&other)?;
    std::fs::write(other.join("sunshine.conf"), "encoder=software")?;
    found.legacy.push(detect::Product {
        location: Some(other),
        ..product.clone()
    });
    assert!(migration_source(&found, &profile, &install).is_err());
    found.legacy.pop();
    // The same host listed twice (its MSI and its own entry) is one source.
    found.vibepollo_entries.push(product.clone());
    assert!(migration_source(&found, &profile, &install)?.is_some());
    found.vibepollo_entries.clear();
    // Hosts with nothing to import are replaced as the summary says, and
    // stale entries without a folder do not block setup.
    for location in [None, Some(temp.path().join("deleted"))] {
        found.legacy.push(detect::Product {
            location,
            ..product.clone()
        });
    }
    std::fs::remove_file(old.join("config/sunshine.conf"))?;
    assert_eq!(migration_source(&found, &profile, &install)?, None);
    // Even then Rubylight is not installed where a host is removed.
    assert!(migration_source(&found, &profile, &old.join("nested")).is_err());
    std::fs::write(profile.join("sunshine_state.json"), "existing pairings")?;
    assert_eq!(migration_source(&found, &profile, &install)?, None);
    assert_eq!(
        migration_source(&detect::Found::default(), &profile, &install)?,
        None
    );
    Ok(())
}
#[test]
fn an_install_that_fails_after_the_import_can_be_run_again() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir()?;
    let old = temp.path().join("Vibepollo");
    let profile = temp.path().join("Butterpollo/config");
    let install = temp.path().join("programs/Butterpollo");
    std::fs::create_dir_all(old.join("config"))?;
    std::fs::write(old.join("config/sunshine.conf"), "encoder=amdvce")?;
    let found = detect::Found {
        legacy: vec![detect::Product {
            key: "Vibepollo".into(),
            name: "Vibepollo".into(),
            version: "2.0.0".into(),
            location: Some(old.clone()),
            uninstall: None,
            quiet_uninstall: None,
            msi: false,
            root: HKEY_LOCAL_MACHINE,
        }],
        ..Default::default()
    };
    let import = || -> Result<()> {
        std::fs::create_dir_all(profile.join("credentials"))?;
        std::fs::write(profile.join("sunshine.conf"), "encoder=amdvce")?;
        std::fs::write(profile.join("credentials/cakey.pem"), "key")?;
        Ok(())
    };
    assert!(migration_source(&found, &profile, &install)?.is_some());
    // The file copy after the import fails: the error is kept and the
    // import is gone, so the retry imports again.
    let error = undo_import_on_error(&profile, true, || {
        import()?;
        bail!("disk full")
    })
    .unwrap_err();
    assert_eq!(format!("{error:#}"), "disk full");
    assert!(!profile.exists());
    assert!(migration_source(&found, &profile, &install)?.is_some());
    // A successful run keeps it.
    undo_import_on_error(&profile, true, import)?;
    assert!(profile.join("sunshine.conf").is_file());
    // Without an import nothing is removed.
    assert!(undo_import_on_error(&profile, false, || bail!("disk full")).is_err());
    assert!(profile.join("sunshine.conf").is_file());
    // An import that cannot be removed says so and where.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(profile.join("credentials/cakey.pem"))?;
    let error = undo_import_on_error(&profile, true, || bail!("disk full")).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.starts_with("disk full"), "{message}");
    assert!(
        message.contains(&profile.display().to_string()),
        "{message}"
    );
    drop(held);
    Ok(())
}
#[test]
fn legacy_migrations_keep_complete_profiles_and_own_configured_files_and_covers() -> Result<()> {
    use crate::profile_tests::{assert_usable, package, rc23_profile, snapshot};
    use butterpollo_config::config::Config;
    use butterpollo_session::{migration, state};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    let temp = tempfile::tempdir()?;
    let template = temp.path().join("rc.23/config");
    rc23_profile(&template)?;
    let staged = temp.path().join("staged");
    let entries = package(&staged, env!("CARGO_PKG_VERSION"), "new.dll")?;
    for family in ["Vibepollo", "Apollo", "Sunshine"] {
        for external in [false, true] {
            let case = temp.path().join(format!("{family}-{external}"));
            let old = case.join(family);
            let source = if family == "Sunshine" {
                old.clone()
            } else {
                old.join("config")
            };
            let profile = case.join("Butterpollo/config");
            let install = case.join("programs/Butterpollo");
            copy_tree(&template, &source)?;
            let mut apps = state::load_json(&source.join("apps.json"), Value::Null)?;
            apps["apps"][1]["image-path"] = json!(source.join("covers/custom/Épopée.png"));
            state::write_json(&source.join("apps.json"), &apps)?;
            let mut config = Config::load(&source.join("sunshine.conf"))?;
            config.values.insert(
                "log_path".into(),
                source.join("butterpollo.log").display().to_string(),
            );
            let configured = [
                ("file_state", "sunshine_state.json"),
                ("file_apps", "apps.json"),
                ("vibeshine_file_state", "vibeshine_state.json"),
                ("credentials_file", "sunshine_credentials.json"),
                ("cert", "credentials/cacert.pem"),
                ("pkey", "credentials/cakey.pem"),
            ];
            if external {
                let shared = old.join("shared files");
                for (key, name) in configured {
                    let target = shared.join(name);
                    replace_file(&source.join(name), &target)?;
                    config
                        .values
                        .insert(key.into(), target.display().to_string());
                }
                let cover = shared.join("Épopée.png");
                replace_file(&source.join("covers/custom/Épopée.png"), &cover)?;
                apps["apps"][1]["image-path"] = json!(cover);
                state::write_json(&shared.join("apps.json"), &apps)?;
            } else if family == "Sunshine" {
                // Sunshine can keep the web password in the paired-state document.
                std::fs::remove_file(source.join("sunshine_credentials.json"))?;
            }
            std::fs::write(source.join("sunshine.conf"), config.text())?;
            let source_before = snapshot(&old)?;
            let mut expected = snapshot(&source)?;
            expected.retain(|name, _| !name.starts_with("logs"));
            for (key, name) in configured {
                if external {
                    expected.insert(name.into(), std::fs::read(config.path(key, &source, name))?);
                    config.values.insert(key.into(), name.into());
                }
            }
            config
                .values
                .insert("log_path".into(), "butterpollo.log".into());
            expected.insert("sunshine.conf".into(), config.text().into_bytes());
            let product = detect::Product {
                key: family.into(),
                name: family.into(),
                version: "2.0.0".into(),
                location: Some(old.clone()),
                uninstall: None,
                quiet_uninstall: None,
                msi: false,
                root: HKEY_LOCAL_MACHINE,
            };
            let found = detect::Found {
                legacy: vec![product],
                ..Default::default()
            };
            let selected = migration_source(&found, &profile, &install)?.unwrap();
            assert_eq!(selected, system::win32_path(&old.canonicalize()?)?);
            let selected_profile = if selected.join("config/sunshine.conf").is_file() {
                selected.join("config")
            } else {
                selected
            };
            // --import-config in the staged host calls this exact importer.
            migration::import(&selected_profile, &profile)?;
            copy_package(&staged, &install, &entries)?;
            for app in apps["apps"].as_array_mut().unwrap() {
                let original = source.join(app["image-path"].as_str().unwrap());
                let bytes = std::fs::read(original)?;
                let name =
                    PathBuf::from("covers").join(format!("{:x}.png", Sha256::digest(&bytes)));
                app["image-path"] = json!(profile.canonicalize()?.join(&name));
                expected.insert(name, bytes);
            }
            expected.insert("apps.json".into(), serde_json::to_vec_pretty(&apps)?);
            assert_eq!(
                snapshot(&profile)?,
                expected,
                "{family}, external={external}"
            );
            assert_eq!(snapshot(&old)?, source_before);
            assert_usable(&profile)?;
            assert!(migration_source(&found, &profile, &install).is_err());
            assert!(migration::import(&source, &profile).is_err());
            assert_eq!(snapshot(&profile)?, expected);
            assert_eq!(snapshot(&old)?, source_before);
            assert!(old.canonicalize()?.starts_with(temp.path().canonicalize()?));
            std::fs::remove_dir_all(&old)?;
            assert_usable(&profile)?;
            for app in apps["apps"].as_array().unwrap() {
                let cover = PathBuf::from(app["image-path"].as_str().unwrap());
                let relative = cover.strip_prefix(profile.canonicalize()?)?;
                assert_eq!(std::fs::read(&cover)?, expected[relative]);
            }
            assert_eq!(snapshot(&profile)?, expected);
        }
    }
    Ok(())
}

#[test]
fn the_import_error_and_its_causes_are_shown() {
    let output = "2026-10-07T10:00:00Z  WARN butterpollo_session::migration: profile file not imported file=C:\\old\\dump.bin reason=\"it is larger than 64 MiB\"\n\
Error: the settings imported from C:\\old would keep Rubylight from starting\n\
\n\
Caused by:\n    0: invalid JSON in C:\\ProgramData\\Butterpollo\\.butterpollo-import-1\\vibeshine_state.json\n    1: expected value at line 1 column 1\n";
    assert_eq!(
        import_error(output).unwrap(),
        "the settings imported from C:\\old would keep Rubylight from starting\n\n\
Caused by:\n    0: invalid JSON in C:\\ProgramData\\Butterpollo\\.butterpollo-import-1\\vibeshine_state.json\n    1: expected value at line 1 column 1"
    );
    assert_eq!(
        import_error("Error: no sunshine.conf\n").unwrap(),
        "no sunshine.conf"
    );
    assert!(import_error("thread 'main' panicked\n").is_none());
    assert!(import_error("").is_none());
}
#[test]
fn a_failed_stop_aborts_installation_and_restarts_the_previous_services() {
    let mut stopped = Vec::new();
    let mut started = Vec::new();
    let result = stop_running(
        &OLD_SERVICES,
        |_| true,
        |service| {
            stopped.push(service.to_owned());
            if service == "SunshineService" {
                bail!("the service did not stop");
            }
            Ok(())
        },
        |service: &str| {
            started.push(service.to_owned());
            Ok(())
        },
    );
    assert!(result.is_err());
    drop(result);
    assert_eq!(stopped, ["ApolloService", "SunshineService"]);
    assert_eq!(started, stopped);
}
#[test]
fn only_previously_running_services_restart_on_failure() -> Result<()> {
    let mut started = Vec::new();
    let restart = stop_running(
        &OLD_SERVICES,
        |service| service == "ApolloService",
        |_| Ok(()),
        |service: &str| {
            started.push(service.to_owned());
            Ok(())
        },
    )?;
    drop(restart);
    assert_eq!(started, ["ApolloService"]);
    let mut restart = Restart {
        services: vec!["ApolloService"],
        start: |_: &str| panic!("successful installation must not restart twice"),
    };
    restart.services.clear();
    drop(restart);
    Ok(())
}
/// Setup staged the package in ProgramData\Butterpollo\setup and ran the
/// staged host from there elevated; Defender's cloud model quarantined
/// the staged service. It stages in the installation folder now, which
/// it secures first, and leaves nothing behind.
#[test]
fn the_package_is_staged_in_the_installation_folder() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let install = temp.path().join("Rubylight");
    let created = create_install(&install)?;
    assert!(created);
    let staging = prepare_staging(&install, created)?;
    assert_eq!(staging.path(), install.join(".staging").join("package"));
    assert!(
        !staging
            .path()
            .starts_with(system::program_data().join("Butterpollo"))
    );
    system::trusted_tree(&staging_root(&install), &[])?;
    std::fs::write(staging.path().join("butterpollo.exe"), b"staged")?;
    drop(staging);
    // The folder this run created goes with the staging folder.
    assert!(!install.exists());

    std::fs::create_dir(&install)?;
    std::fs::write(install.join("own.txt"), b"the user's")?;
    drop(prepare_staging(&install, true)?);
    assert!(!staging_root(&install).exists());
    assert_eq!(std::fs::read(install.join("own.txt"))?, b"the user's");
    Ok(())
}
#[test]
fn an_interrupted_setups_staging_is_replaced_and_update_backups_kept() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let install = temp.path().join("Rubylight");
    create_install(&install)?;
    let root = staging_root(&install);
    std::fs::create_dir_all(root.join("package"))?;
    std::fs::write(root.join("package").join("stale.exe"), b"old")?;
    let backup = root.join("transaction-1-2").join("previous");
    std::fs::create_dir_all(&backup)?;
    std::fs::write(backup.join("butterpollo.exe"), b"previous")?;
    let staging = prepare_staging(&install, false)?;
    assert!(!staging.path().join("stale.exe").exists());
    system::trusted_tree(&root, &[])?;
    drop(staging);
    assert_eq!(std::fs::read(backup.join("butterpollo.exe"))?, b"previous");
    Ok(())
}
#[test]
fn a_link_in_the_staging_folder_is_refused_not_followed() -> Result<()> {
    if !system::elevated() {
        return Ok(());
    }
    let temp = tempfile::tempdir_in(system::program_data())?;
    system::restrict(temp.path(), true)?;
    let install = temp.path().join("Rubylight");
    create_install(&install)?;
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(outside.join("package"))?;
    std::fs::write(outside.join("file.dll"), b"unchanged")?;
    std::fs::write(outside.join("package").join("file.dll"), b"unchanged")?;
    let root = staging_root(&install);
    for link in [root.clone(), root.join("package")] {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::os::windows::fs::symlink_dir(&outside, &link)?;
        let error = prepare_staging(&install, false)
            .err()
            .context("a link was followed")?;
        assert!(format!("{error:#}").contains("reparse point"), "{error:#}");
        for file in [
            outside.join("file.dll"),
            outside.join("package").join("file.dll"),
        ] {
            assert_eq!(std::fs::read(file)?, b"unchanged");
        }
        std::fs::remove_dir(&link)?;
    }
    Ok(())
}
#[test]
fn the_earlier_staging_folder_in_program_data_is_removed() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let setup = temp.path().join("Butterpollo").join("setup");
    let profile = temp.path().join("Butterpollo").join("config");
    std::fs::create_dir_all(setup.join("drivers"))?;
    std::fs::create_dir_all(&profile)?;
    std::fs::write(setup.join("butterpollo-service.exe"), b"staged")?;
    std::fs::write(profile.join("sunshine.conf"), b"kept")?;
    remove_legacy_staging(temp.path())?;
    assert!(!setup.exists());
    assert_eq!(std::fs::read(profile.join("sunshine.conf"))?, b"kept");
    // Nothing to remove is no error.
    remove_legacy_staging(temp.path())?;
    if system::elevated() {
        // A link there goes itself; what it leads to stays.
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("file"), b"unchanged")?;
        std::os::windows::fs::symlink_dir(&outside, &setup)?;
        remove_legacy_staging(temp.path())?;
        assert!(std::fs::symlink_metadata(&setup).is_err());
        assert_eq!(std::fs::read(outside.join("file"))?, b"unchanged");
    }
    Ok(())
}
