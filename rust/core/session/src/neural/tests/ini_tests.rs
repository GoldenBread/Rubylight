//! OptiScaler.ini frame-generation override, restore record, and `OptiDllPath`
//! resolution tests for Neural Rendering.
use super::{ini::Ini, *};

/// An ini as `lines` joined with `ending`, a BOM in front when `bom`.
fn ini(lines: &[&str], ending: &str, bom: bool, trailing: bool) -> Vec<u8> {
    let mut text = lines.join(ending);
    if trailing {
        text.push_str(ending);
    }
    let mut bytes = if bom {
        b"\xef\xbb\xbf".to_vec()
    } else {
        Vec::new()
    };
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

fn values(bytes: &[u8]) -> Vec<Option<String>> {
    let ini = Ini::read(bytes).unwrap();
    INI_VALUES
        .iter()
        .map(|(key, _)| ini.get(INI_SECTION, key).unwrap())
        .collect()
}

#[test]
fn the_addon_sets_three_values_and_puts_back_exactly_what_was_there() {
    let stock = [
        "; OptiScaler",
        "[FrameGen]",
        "; Selected FG Input",
        "FGInput = auto",
        "",
        "FGNvngxReplacement=auto",
        "",
        "DisableHudless=auto",
        "",
        // As the release's OptiScaler.ini has them: [DLSSG] without the key,
        // [NvngxFG] with its own DisableHudless.
        "[DLSSG]",
        "Enabled=auto",
        "",
        "[NvngxFG]",
        "DisableHudless = auto",
        "",
        "[Libraries]",
        "OptiDllPath=auto",
    ];
    let partial = [
        "[FrameGen]",
        "Enabled=true",
        "",
        "[Libraries]",
        "OptiDllPath=auto",
    ];
    let none = ["[Upscalers]", "Dx12Upscaler=fsr31"];
    // The section's header is the last line, which may end the file with no
    // line break after it.
    let header_last = ["[Upscalers]", "Dx12Upscaler=fsr31", "[FrameGen]"];
    for lines in [
        &stock[..],
        &partial[..],
        &none[..],
        &header_last[..],
        &[][..],
    ] {
        for ending in ["\r\n", "\n"] {
            for bom in [false, true] {
                for trailing in [false, true] {
                    let before = ini(lines, ending, bom, trailing);
                    let (after, record) = ini_apply(&before, "OptiScaler.ini", None).unwrap();
                    assert_eq!(
                        values(&after),
                        [
                            Some("nvngxfg".into()),
                            Some(PROVIDER.into()),
                            Some("false".into())
                        ],
                        "{lines:?} {ending:?}"
                    );
                    // Its line endings and byte order mark stay.
                    assert_eq!(after.starts_with(b"\xef\xbb\xbf"), bom);
                    let text = String::from_utf8_lossy(&after);
                    if ending == "\r\n" && !lines.is_empty() {
                        let bare = text.replace("\r\n", "");
                        assert!(!bare.contains('\n') && !bare.contains('\r'), "{text:?}");
                    }
                    // The other section's DisableHudless is not touched.
                    if lines == stock {
                        assert!(text.contains("DisableHudless = auto"), "{text:?}");
                    }
                    let restored = ini_restore(&after, &record).unwrap();
                    assert_eq!(
                        String::from_utf8_lossy(&restored),
                        String::from_utf8_lossy(&before),
                        "{lines:?} {ending:?} bom {bom} trailing {trailing}"
                    );
                    // A second apply keeps the first record.
                    let (again, kept) = ini_apply(&after, "OptiScaler.ini", Some(&record)).unwrap();
                    assert_eq!(again, after);
                    assert_eq!(kept, record);
                }
            }
        }
    }
    // OptiScaler's menu changed a value since: it is put back all the same.
    let before = ini(&stock, "\r\n", false, true);
    let (after, record) = ini_apply(&before, "OptiScaler.ini", None).unwrap();
    let edited = String::from_utf8(after)
        .unwrap()
        .replace("FGInput=nvngxfg", "FGInput=fsrfg");
    assert_eq!(ini_restore(edited.as_bytes(), &record).unwrap(), before);
    // What it can't change and put back exactly, it refuses.
    assert!(ini_apply(b"\xff\xfe[\x00", "OptiScaler.ini", None).is_err());
    let twice = ini(&["[FrameGen]", "FGInput=a", "FGInput=b"], "\n", false, true);
    assert!(
        ini_apply(&twice, "OptiScaler.ini", None)
            .unwrap_err()
            .contains("twice")
    );
    let sections = ini(&["[FrameGen]", "[framegen]"], "\n", false, true);
    assert!(ini_apply(&sections, "OptiScaler.ini", None).is_err());
}

#[test]
fn optiscalers_folder_comes_from_its_ini_and_stays_in_the_game_folder() {
    let with = |value: &str| {
        ini(
            &["[Libraries]", &format!("OptiDllPath={value}")],
            "\r\n",
            false,
            true,
        )
    };
    assert_eq!(opti_dir(&with("auto"), true).as_deref(), Ok("OptiScaler"));
    assert_eq!(opti_dir(&with("auto"), false).as_deref(), Ok(""));
    assert_eq!(opti_dir(b"[FrameGen]\n", true).as_deref(), Ok("OptiScaler"));
    assert_eq!(
        opti_dir(&with(".\\Mods\\Opti\\"), false).as_deref(),
        Ok("Mods/Opti")
    );
    assert_eq!(
        opti_dir(&with("\".\\OptiScaler\""), false).as_deref(),
        Ok("OptiScaler")
    );
    assert_eq!(opti_dir(&with("."), true).as_deref(), Ok(""));
    for outside in [
        "C:\\Mods",
        "..\\Mods",
        "\\\\server\\share",
        "/x",
        "a/../../b",
    ] {
        assert!(opti_dir(&with(outside), true).is_err(), "{outside}");
    }
}
