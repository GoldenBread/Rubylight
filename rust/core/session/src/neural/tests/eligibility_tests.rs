//! Game eligibility, proxy selection, anti-cheat and mod detection, folder ACL
//! verdict, and switch job state machine tests for Neural Rendering.
use super::*;
use serde_json::json;

#[test]
fn optiscaler_never_takes_a_name_another_program_holds() {
    let with = |vulkan: bool, taken: &'static [(&'static str, Occupant)]| {
        proxy(vulkan, move |name| {
            taken
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(Occupant::Free, |(_, o)| *o)
        })
    };
    assert_eq!(with(false, &[]), Ok("dxgi.dll"));
    // ReShade or a game's own DLL as dxgi.dll: the next name.
    assert_eq!(
        with(false, &[("dxgi.dll", Occupant::Foreign)]),
        Ok("winmm.dll")
    );
    assert_eq!(
        with(
            false,
            &[
                ("dxgi.dll", Occupant::Foreign),
                ("winmm.dll", Occupant::Foreign)
            ]
        ),
        Ok("version.dll")
    );
    // A game that only uses Vulkan never loads dxgi.dll.
    assert_eq!(with(true, &[]), Ok("winmm.dll"));
    assert_eq!(
        with(true, &[("winmm.dll", Occupant::Foreign)]),
        Ok("version.dll")
    );
    let vulkan = with(
        true,
        &[
            ("winmm.dll", Occupant::Foreign),
            ("version.dll", Occupant::Foreign),
        ],
    )
    .unwrap_err();
    assert!(
        vulkan.contains("every name") && !vulkan.contains("dxgi"),
        "{vulkan}"
    );
    // An earlier install keeps its name.
    assert_eq!(
        with(false, &[("version.dll", Occupant::Ours)]),
        Ok("version.dll")
    );
    assert!(
        with(
            false,
            &[
                ("dxgi.dll", Occupant::Foreign),
                ("winmm.dll", Occupant::Foreign),
                ("version.dll", Occupant::Foreign)
            ]
        )
        .unwrap_err()
        .contains("every name")
    );
}

#[test]
fn a_vulkan_game_and_an_optiscaler_with_providers_are_told_by_their_bytes() {
    let wide = |text: &str| text.bytes().flat_map(|b| [b, 0]).collect::<Vec<u8>>();
    assert!(vulkan_only(b"..VULKAN-1.dll..kernel32.dll"));
    assert!(vulkan_only(&wide("vulkan-1.dll")));
    assert!(!vulkan_only(b"vulkan-1.dll d3d12.dll"));
    assert!(!vulkan_only(
        &[b"vulkan-1.dll ".as_slice(), &wide("DXGI.dll")].concat()
    ));
    assert!(!vulkan_only(b"d3d11.dll"));
    assert!(custom_providers(
        b"..LoadLibraries Custom DLSSG {} initialized.."
    ));
    assert!(!custom_providers(b"FGNvngxReplacement Nukems Arturs"));
    assert!(is_optiscaler_binary(b"xxOptiScalerxx"));
    assert!(!is_optiscaler_binary(b"plain dll"));
    assert!(dlss_frame_generation("nvngx_dlssg.dll") && dlss_frame_generation("SL.DLSS_G.DLL"));
    assert!(!dlss_frame_generation("nvngx_dlss.dll") && !dlss_frame_generation("sl.dlss.dll"));
}

#[test]
fn other_upscaler_mods_are_recognised_by_their_files() {
    assert_eq!(other_mod("OptiScaler.ini"), Some("OptiScaler"));
    assert_eq!(other_mod("OptiScaler"), Some("OptiScaler"));
    assert_eq!(other_mod("nvngx.ini"), Some("OptiScaler"));
    assert_eq!(
        other_mod("fakenvapi.dll"),
        Some("fakenvapi (an OptiScaler add-on)")
    );
    assert_eq!(other_mod("dlss-enabler-upscaler.dll"), Some("DLSS Enabler"));
    assert_eq!(
        other_mod("dlssg_to_fsr3_amd_is_better.dll"),
        Some("dlssg-to-fsr3")
    );
    assert_eq!(other_mod("SpecialK64.dll"), Some("Special K"));
    for name in [
        "ReShade.ini",
        "dxgi.dll",
        "nvngx_dlss.dll",
        "Game.exe",
        "OptiScaler.log",
        MANIFEST,
        LEGACY_MANIFEST,
    ] {
        assert_eq!(other_mod(name), None, "{name}");
    }
}

#[test]
fn anti_cheat_is_recognised_by_its_files_and_folders() {
    for (name, which) in [
        ("EasyAntiCheat", "Easy Anti-Cheat"),
        ("EasyAntiCheat_EOS", "Easy Anti-Cheat"),
        ("EasyAntiCheat_x64.dll", "Easy Anti-Cheat"),
        ("start_protected_game.exe", "Easy Anti-Cheat"),
        ("BattlEye", "BattlEye"),
        ("BEService_x64.exe", "BattlEye"),
        ("BEClient_x64.dll", "BattlEye"),
        ("RainbowSix_BE.exe", "BattlEye"),
        ("vgk.sys", "Riot Vanguard"),
        ("randgrid.sys", "Ricochet"),
        ("mhyprot3.sys", "mhyprot"),
        ("GameGuard", "nProtect GameGuard"),
        ("npggNT.des", "nProtect GameGuard"),
        ("XIGNCODE", "XIGNCODE3"),
        ("x3.xem", "XIGNCODE3"),
        ("PnkBstrA.exe", "PunkBuster"),
        ("EQU8", "EQU8"),
        ("faceit.sys", "FACEIT"),
        ("AntiCheatExpert", "Anti-Cheat Expert"),
        ("SGuard64.exe", "Anti-Cheat Expert"),
        ("EAAntiCheat.GameServiceLauncher.exe", "EA Javelin"),
        ("denuvo-anti-cheat.sys", "Denuvo Anti-Cheat"),
        ("NeacSafe64.sys", "NetEase Anti-Cheat"),
        ("HShield", "AhnLab HackShield"),
    ] {
        assert_eq!(anti_cheat(name), Some(which), "{name}");
    }
    for name in [
        "Binaries",
        "Win64",
        "SilentHill2-Win64-Shipping.exe",
        "Engine",
        "be.exe",
        "Tribe.exe",
        "steam_api64.dll",
        "dxgi.dll",
        "anticheat_readme_not.txt",
        "Faceless.pak",
    ] {
        assert_eq!(anti_cheat(name), None, "{name}");
    }
}

#[test]
fn non_game_executables_acl_verdict_and_switch_state_machine() {
    assert_eq!(classify_non_game_exe("Steam.exe"), Some("a launcher"));
    assert_eq!(
        classify_non_game_exe("Playnite.FullscreenApp.exe"),
        Some("a launcher")
    );
    assert_eq!(
        classify_non_game_exe("python3.exe"),
        Some("a script host or interpreter")
    );
    assert_eq!(classify_non_game_exe("Cyberpunk2077.exe"), None);
    assert!(is_helper_program("crashreporter.exe") && !is_helper_program("game.exe"));
    assert!(is_shared_folder_name("Program Files") && !is_shared_folder_name("MyGame"));

    assert_eq!(game_why(&format!("{NO_DLSS} here")), "no_frame_generation");
    assert_eq!(
        game_why(&format!("uses EAC, and {ANTI_CHEAT}")),
        "anti_cheat"
    );
    assert_eq!(game_why("another mod is installed"), "game");

    assert_eq!(
        without_runtime(vec!["A.dll", "B.dll"], &["A.dll"]),
        vec!["B.dll"]
    );

    let ok_sec = Security {
        link: false,
        owner: "S-1-5-32-544".into(),
        writers: vec!["S-1-5-18".into()],
    };
    assert_eq!(security_verdict("C:\\Games", &ok_sec, |s| s.into()), Ok(()));
    let link_sec = Security {
        link: true,
        owner: "S-1-5-18".into(),
        writers: vec![],
    };
    assert!(
        security_verdict("C:\\Games", &link_sec, |s| s.into())
            .unwrap_err()
            .contains("link")
    );
    let user_writer = Security {
        link: false,
        owner: "S-1-5-18".into(),
        writers: vec!["S-1-5-32-545".into()],
    };
    assert!(
        security_verdict("C:\\Games", &user_writer, |_| "BUILTIN\\Users".into())
            .unwrap_err()
            .contains("BUILTIN\\Users")
    );

    let mut job = Job::new("install", "download");
    assert!(job.is_running());
    job.phase = "done";
    assert!(!job.is_running());

    assert_eq!(
        enable_preconditions(false, Ok(()), true, false, false),
        Err(Refusal::NotAccepted)
    );
    assert_eq!(
        enable_preconditions(true, Err("old gpu".into()), true, false, false),
        Err(Refusal::Unavailable("old gpu".into()))
    );
    assert_eq!(
        enable_preconditions(true, Ok(()), false, false, false),
        Err(Refusal::NotFound)
    );
    assert_eq!(
        enable_preconditions(true, Ok(()), true, true, false),
        Err(Refusal::Unavailable(ALREADY_ON.into()))
    );
    assert_eq!(
        enable_preconditions(true, Ok(()), true, false, true),
        Err(Refusal::Streaming)
    );
    assert_eq!(
        enable_preconditions(true, Ok(()), true, false, false),
        Ok(())
    );
    assert!(should_run_game_check(true, false, true, false));
    assert!(!should_run_game_check(true, true, true, false));

    let capable = Capable {
        gpu: "AMD Radeon RX 7900 XT".into(),
        streams: None,
    };
    let check = GameCheck::Choose("pick exe".into());
    let payload = status_json(StatusInput {
        accepted: true,
        capability: Ok(&capable),
        found: true,
        setting: None,
        legacy: None,
        game: Some(&check),
        job: Some(&job),
    });
    assert_eq!(payload["available"], json!(true));
    assert_eq!(payload["game"]["why"], json!("choose_program"));
    assert_eq!(payload["job"]["phase"], json!("done"));
}
