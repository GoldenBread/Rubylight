//! Nightjar file transfer, shared folders, image clipboard, and Sleep PC interop tests.
mod common;

use anyhow::Result;
use butterpollo_session::{
    native::{FILE_DOWNLOAD, FILE_UPLOAD},
    power_action::SleepStates,
};
use butterpollo_stream::stream::reconnect_harness::{Harness, until};
use common::{LOADED, device, paired, stream};
use nightjar_core::http::{
    Availability, EntryKind, Feature, FileError, FolderAccess, HostClient, PowerError, Reason,
    Refusal, TransferCancel, transfers::Problem,
};
use rubylight_native_api::feature;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// The transfer folder and the setting naming it.
struct Folder(PathBuf);
impl Folder {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "butterpollo-interop-files-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn setting(&self) -> String {
        format!(
            "file_transfer=true\nfile_transfer_dir={}\n",
            self.0.display()
        )
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn upload(client: &HostClient, name: &str, bytes: &[u8]) -> Result<u64, FileError> {
    let uploaded = client.upload_file(
        name,
        &mut &bytes[..],
        bytes.len() as u64,
        &mut |_, _| {},
        &TransferCancel::new(),
    )?;
    assert_eq!(uploaded.name, name);
    Ok(uploaded.size)
}
/// Downloads `name`, which the list gave as `listed` bytes, as Nightjar does.
fn download(client: &HostClient, name: &str, listed: u64) -> Result<Vec<u8>, FileError> {
    let mut bytes = vec![];
    let length = client.download_file(
        name,
        listed,
        &mut bytes,
        &mut |_, _| {},
        &TransferCancel::new(),
    )?;
    assert_eq!(length, bytes.len() as u64);
    Ok(bytes)
}
/// Zeros, until `left` bytes were read; then it cancels `stop` and reads on, as a user who
/// cancels while the file is sent.
struct CancelAfter<'a> {
    left: usize,
    stop: &'a TransferCancel,
}
impl std::io::Read for CancelAfter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            self.stop.cancel();
        }
        let n = buf.len().min(self.left.max(1));
        self.left = self.left.saturating_sub(n);
        buf[..n].fill(0);
        Ok(n)
    }
}
/// The status and error code of a refusal, as the host sent them.
fn refusal<T: std::fmt::Debug>(result: Result<T, FileError>) -> (u16, &'static str) {
    match result {
        Err(FileError::Refused {
            refusal, status, ..
        }) => (status, refusal.code()),
        other => panic!("not a refusal: {other:?}"),
    }
}

/// Files through the native API: an upload Nightjar lists and downloads again, and each
/// refusal's status and code as Nightjar reads them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_transfers_files_and_reads_each_refusal() -> Result<()> {
    let folder = Folder::new()?;
    let h = Harness::configured_with(
        10_000,
        1,
        &format!("{}file_transfer_max_mb=1\n", folder.setting()),
        Some(butterpollo_web::native_api::router()),
    )
    .await?;
    let client = device(&h, u32::MAX, &[])?;
    let info = client.server_info(true)?;
    let native = client.native_capabilities_with_files()?;
    assert!(native.files.file_upload && native.files.file_download);
    let capabilities = info.capabilities.with_native(&native);
    assert!(capabilities.has(Feature::FileUpload) && capabilities.has(Feature::FileDownload));

    assert_eq!(upload(&client, "notes.txt", b"from Nightjar")?, 13);
    assert_eq!(std::fs::read(folder.0.join("notes.txt"))?, b"from Nightjar");
    let list = client.list_files()?;
    assert_eq!(list.max_file_bytes, 1 << 20);
    let listed: Vec<_> = list
        .files
        .iter()
        .map(|f| (f.name.as_str(), f.size))
        .collect();
    assert_eq!(listed, [("notes.txt", 13)]);
    assert_eq!(download(&client, "notes.txt", 13)?, b"from Nightjar");

    // Sent again: the host keeps both and answers with the name it gave (Nightjar's "Sent as").
    let again = client.upload_file(
        "notes.txt",
        &mut &b"again"[..],
        5,
        &mut |_, _| {},
        &TransferCancel::new(),
    )?;
    assert_eq!(again.name, "notes (1).txt");
    // Into a file here, as Nightjar for Windows downloads: whole under its name, and nothing
    // left behind by a refused one.
    let here = Folder::new()?;
    let dest = here.0.join("notes.txt");
    let saved = client.download_to_path(
        "notes.txt",
        13,
        &dest,
        &mut |_, _| {},
        &TransferCancel::new(),
    )?;
    assert_eq!(saved, 13);
    assert_eq!(std::fs::read(&dest)?, b"from Nightjar");
    // A file that grew past its listed size since the list is refused, and leaves nothing.
    let listed_short = client.download_to_path(
        "notes.txt",
        4,
        &here.0.join("short.txt"),
        &mut |_, _| {},
        &TransferCancel::new(),
    );
    assert!(listed_short.is_err(), "{listed_short:?}");
    let missing = here.0.join("missing.txt");
    assert_eq!(
        refusal(client.download_to_path(
            "missing.txt",
            0,
            &missing,
            &mut |_, _| {},
            &TransferCancel::new()
        )),
        (404, "not_found")
    );
    assert_eq!(names(&here.0)?, ["notes.txt"]);
    // Cancelled part way: no problem to show, and the host drops what arrived.
    let stop = TransferCancel::new();
    let mut reader = CancelAfter {
        left: 64 << 10,
        stop: &stop,
    };
    let cancelled = client.upload_file(
        "cancelled.bin",
        &mut reader,
        512 << 10,
        &mut |_, _| {},
        &stop,
    );
    assert!(
        matches!(&cancelled, Err(e) if Problem::of(e).is_none()),
        "{cancelled:?}"
    );
    until("the cancelled upload's part file gone", LOADED, || {
        names(&folder.0).is_ok_and(|names| names == ["notes (1).txt", "notes.txt"])
    })
    .await;

    assert_eq!(
        refusal(download(&client, "missing.txt", 0)),
        (404, "not_found")
    );
    assert_eq!(refusal(upload(&client, "CON", b"x")), (400, "invalid_name"));
    assert_eq!(
        refusal(upload(&client, "big.bin", &vec![0; (1 << 20) + 1])),
        (413, "too_large")
    );
    // Free space down to the reserve the host keeps.
    h.h.file_transfers.fake_space(1 << 30, 10 << 30);
    assert_eq!(
        refusal(upload(&client, "full.txt", b"x")),
        (507, "insufficient_storage")
    );
    h.h.file_transfers.fake_space(100 << 30, 200 << 30);

    // Without the Upload files permission, downloads go on.
    paired(&h, |c| c.perm &= !FILE_UPLOAD);
    assert_eq!(
        refusal(upload(&client, "later.txt", b"x")),
        (403, "forbidden")
    );
    assert_eq!(download(&client, "notes.txt", 13)?, b"from Nightjar");
    let capabilities = info
        .capabilities
        .with_native(&client.native_capabilities_with_files()?);
    assert_eq!(
        capabilities.availability(Feature::FileUpload),
        Availability::Unsupported(Some(Reason::FilePermission))
    );
    assert!(capabilities.has(Feature::FileDownload));
    paired(&h, |c| c.perm &= !FILE_DOWNLOAD);
    assert_eq!(refusal(client.list_files()), (403, "forbidden"));
    assert_eq!(
        refusal(download(&client, "notes.txt", 13)),
        (403, "forbidden")
    );
    assert!(!folder.0.join("later.txt").exists() && !folder.0.join("big.bin").exists());
    Ok(())
}

/// With File transfer off every route answers `file_transfer_disabled`, and the native API
/// lists no file features.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_reads_file_transfer_turned_off() -> Result<()> {
    let h = Harness::configured_with(10_000, 1, "", Some(butterpollo_web::native_api::router()))
        .await?;
    let client = device(&h, u32::MAX, &[])?;
    let native = client.native_capabilities_with_files()?;
    assert!(
        !native
            .capabilities
            .features
            .iter()
            .any(|f| f == feature::FILES_UPLOAD || f == feature::FILES_DOWNLOAD)
    );
    let disabled = (403, "file_transfer_disabled");
    assert_eq!(refusal(client.list_files()), disabled);
    assert_eq!(refusal(upload(&client, "notes.txt", b"x")), disabled);
    assert_eq!(refusal(download(&client, "notes.txt", 13)), disabled);
    Ok(())
}

/// The names in `dir`, `.part` files included.
fn names(dir: &std::path::Path) -> Result<Vec<String>> {
    let mut names = std::fs::read_dir(dir)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}
fn upload_to(client: &HostClient, id: &str, path: &str, bytes: &[u8]) -> Result<String, FileError> {
    let uploaded = client.upload_folder_file(
        id,
        path,
        &mut &bytes[..],
        bytes.len() as u64,
        &mut |_, _| {},
        &TransferCancel::new(),
    )?;
    assert_eq!(uploaded.size, bytes.len() as u64);
    Ok(uploaded.name)
}

/// Shared folders through the native API: Nightjar lists the folders its grants allow,
/// browses one, saves files beside the copies it already has, uploads files the host numbers,
/// and reads each refusal. Nothing is left as `.part` on either side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_browses_and_moves_files_through_shared_folders() -> Result<()> {
    let base = Folder::new()?;
    let (games, music, saved) = (
        base.0.join("Games"),
        base.0.join("Music"),
        base.0.join("saved"),
    );
    for dir in [&games.join("saves"), &music, &saved] {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(games.join("readme.txt"), b"hello")?;
    std::fs::write(games.join("saves").join("slot1.sav"), b"slot")?;
    let h = Harness::configured_with(10_000, 1, "", Some(butterpollo_web::native_api::router()))
        .await?;
    h.h.file_transfers.fake_space(1 << 40, 1 << 41);
    let client = device(&h, u32::MAX, &[])?;
    assert_eq!(refusal(client.shared_folders()), (403, "folders_disabled"));
    let share = serde_json::json!([
        {"id": "games", "name": "Games", "path": games},
        {"id": "music", "name": "Music", "path": music},
    ]);
    h.h.config
        .write()
        .unwrap()
        .values
        .insert("shared_folders".into(), share.to_string());
    // Shared, but not with this device.
    assert_eq!(refusal(client.shared_folders()), (403, "forbidden"));
    let grant = |access: &str| {
        paired(&h, |c| {
            c.extra.insert(
                "shared_folders".into(),
                serde_json::json!({ "games": access }),
            )
        });
    };
    grant("write");
    let capabilities = client
        .server_info(true)?
        .capabilities
        .with_native(&client.native_capabilities_with_files()?);
    assert!(capabilities.has(Feature::SharedFolders));
    let folders = client.shared_folders()?;
    let listed: Vec<_> = folders
        .iter()
        .map(|f| (f.id.as_str(), f.name.as_str(), f.access))
        .collect();
    assert_eq!(listed, [("games", "Games", FolderAccess::Write)]);

    let root = client.folder_entries("games", "")?;
    assert_eq!(root.path, "");
    let entries: Vec<_> = root
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.kind, e.size))
        .collect();
    assert_eq!(
        entries,
        [
            ("saves", EntryKind::Dir, 0),
            ("readme.txt", EntryKind::File, 5)
        ]
    );
    let saves = client.folder_entries("games", "saves")?;
    assert_eq!(saves.path, "saves");
    assert_eq!(saves.entries[0].name, "slot1.sav");

    // Saved twice: the second copy takes the next free name, as the host names uploads.
    let stop = TransferCancel::new();
    let first = client.save_folder_file("games", "readme.txt", 5, &saved, &mut |_, _| {}, &stop)?;
    let second =
        client.save_folder_file("games", "readme.txt", 5, &saved, &mut |_, _| {}, &stop)?;
    assert_eq!(
        (first, second),
        (saved.join("readme.txt"), saved.join("readme (1).txt"))
    );
    assert_eq!(std::fs::read(saved.join("readme (1).txt"))?, b"hello");
    // A file the host doesn't have leaves nothing behind.
    assert_eq!(
        refusal(client.save_folder_file(
            "games",
            "saves/none.sav",
            0,
            &saved,
            &mut |_, _| {},
            &stop
        )),
        (404, "not_found")
    );
    assert_eq!(names(&saved)?, ["readme (1).txt", "readme.txt"]);

    assert_eq!(
        upload_to(&client, "games", "saves/slot2.sav", b"from Nightjar")?,
        "slot2.sav"
    );
    assert_eq!(
        upload_to(&client, "games", "saves/slot2.sav", b"again")?,
        "slot2 (1).sav"
    );
    assert_eq!(
        std::fs::read(games.join("saves").join("slot2.sav"))?,
        b"from Nightjar"
    );
    assert_eq!(
        std::fs::read(games.join("saves").join("slot2 (1).sav"))?,
        b"again"
    );
    assert_eq!(
        names(&games.join("saves"))?,
        ["slot1.sav", "slot2 (1).sav", "slot2.sav"]
    );

    assert_eq!(
        refusal(client.folder_entries("music", "")),
        (403, "forbidden")
    );
    assert_eq!(
        refusal(client.folder_entries("games", "missing")),
        (404, "not_found")
    );
    assert_eq!(
        refusal(upload_to(&client, "games", "saves/CON", b"x")),
        (400, "invalid_name")
    );
    // A read grant lists and downloads, and refuses uploads.
    grant("read");
    assert_eq!(client.shared_folders()?[0].access, FolderAccess::Read);
    let mut bytes = vec![];
    client.download_folder_file("games", "readme.txt", 5, &mut bytes, &mut |_, _| {}, &stop)?;
    assert_eq!(bytes, b"hello");
    assert_eq!(
        refusal(upload_to(&client, "games", "later.txt", b"x")),
        (403, "forbidden")
    );
    assert!(!games.join("later.txt").exists());
    Ok(())
}

/// The image clipboard while Nightjar streams: a PNG it sends lands on the host's clipboard
/// and comes back as the same image. Without a stream of its own, over
/// `clipboard_image_max_mb` or with a broken image, Nightjar reads the host's refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_copies_images_both_ways_while_it_streams() -> Result<()> {
    use butterpollo_session::clipboard_image::{Image, decode_png, encode_png};
    use butterpollo_stream::nvhttp::clipboard;
    use nightjar_core::http::ImageClipboardError;
    let _turn = clipboard::TURN.lock().await;
    *clipboard::PNG.lock().unwrap() = None;
    let h = Harness::configured_with(10_000, 1, "", Some(butterpollo_web::native_api::router()))
        .await?;
    let client = device(&h, u32::MAX, &[])?;
    let capabilities = client
        .server_info(true)?
        .capabilities
        .with_native(&client.native_capabilities_with_files()?);
    assert!(capabilities.has(Feature::ImageClipboard));
    let image = Image {
        width: 2,
        height: 1,
        rgba: vec![255, 0, 0, 255, 0, 0, 255, 128],
    };
    let png = encode_png(&image)?;
    assert!(matches!(
        client.send_clipboard_image(&png),
        Err(ImageClipboardError::Forbidden)
    ));

    let (_, session, stream, _) = stream(&h, &client).await?;
    assert!(client.clipboard_image()?.is_none());
    client.send_clipboard_image(&png)?;
    let copied = client.clipboard_image()?.expect("the image Nightjar sent");
    assert_eq!(decode_png(&copied)?, image);
    // A PNG header the host can't decode the rest of.
    assert!(matches!(
        client.send_clipboard_image(&png[..40]),
        Err(ImageClipboardError::NotPng(_))
    ));
    // Over the host's limit both ways; it refuses the upload by its length, before the body.
    h.h.config
        .write()
        .unwrap()
        .values
        .insert("clipboard_image_max_mb".into(), "1".into());
    let mut large = png.clone();
    large.resize((1 << 20) + 1, 0);
    let sent = client.send_clipboard_image(&large);
    assert!(
        matches!(sent, Err(ImageClipboardError::TooLarge(_))),
        "{sent:?}"
    );
    *clipboard::PNG.lock().unwrap() = Some(large);
    assert!(matches!(
        client.clipboard_image(),
        Err(ImageClipboardError::TooLarge(_))
    ));
    *clipboard::PNG.lock().unwrap() = None;

    stream.stop();
    until("the host's session ended", LOADED, || session.stopping()).await;
    until("sessions released", LOADED, || {
        h.h.sessions.lock().unwrap().idle()
    })
    .await;
    h.finish().await
}

/// A power backend with S3 that counts suspends in place of the PC's.
#[derive(Default)]
struct Suspends(AtomicUsize);
impl butterpollo_stream::power_action::Backend for Suspends {
    fn states(&self) -> Result<SleepStates> {
        Ok(SleepStates {
            s3: true,
            ..Default::default()
        })
    }
    fn suspend(&self) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn power_off(&self, action: butterpollo_session::power_action::Action) -> Result<()> {
        panic!("an interop test reached the {}", action.name())
    }
}

fn power_refusal<T: std::fmt::Debug>(result: Result<T, PowerError>) -> (Refusal, u16) {
    match result {
        Err(PowerError::Refused { refusal, reply }) => (refusal, reply.status),
        other => panic!("not a refusal: {other:?}"),
    }
}

/// Sleep PC: refused until the console allows it for the device, then one token puts the
/// (fake) PC to sleep once and can't be spent again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nightjar_puts_the_host_to_sleep_once_allowed() -> Result<()> {
    let h = Harness::configured_with(10_000, 1, "", Some(butterpollo_web::native_api::router()))
        .await?;
    let suspends = Arc::new(Suspends::default());
    h.h.power.set_backend(suspends.clone());
    let client = device(&h, u32::MAX, &[])?;

    let info = client.server_info(true)?;
    assert_eq!(
        info.capabilities.availability(Feature::Sleep),
        Availability::Unsupported(Some(Reason::SleepPermission))
    );
    let offer = client.power_capabilities()?;
    assert!(offer.sleep_supported && !offer.sleep_allowed && !offer.sleep_available);
    assert_eq!(offer.token, None);
    assert_eq!(
        power_refusal(client.sleep(&"0".repeat(64))),
        (Refusal::Forbidden, 403)
    );

    paired(&h, |c| c.extra.insert("allow_sleep".into(), true.into()));
    assert!(client.server_info(true)?.capabilities.has(Feature::Sleep));
    // The harness runs the desktop app, so the PC is busy: allowed, but no token yet.
    let offer = client.power_capabilities()?;
    assert!(offer.sleep_supported && offer.sleep_allowed && !offer.sleep_available);
    assert_eq!(offer.token, None);

    *h.h.current_app.lock().unwrap() = None;
    let offer = client.power_capabilities()?;
    assert!(offer.sleep_supported && offer.sleep_allowed && offer.sleep_available);
    assert_eq!(offer.active_session_policy, "idle-only");
    let token = offer.token.expect("a token for an allowed, idle host");
    let reply = client.sleep(&token)?;
    assert!(reply.accepted);
    assert_eq!(reply.status, 202);
    until("the suspend after the answer", LOADED, || {
        suspends.0.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        power_refusal(client.sleep(&token)),
        (Refusal::Conflict, 409)
    );
    assert_eq!(suspends.0.load(Ordering::SeqCst), 1);
    Ok(())
}
