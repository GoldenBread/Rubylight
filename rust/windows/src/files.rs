//! Drives on this PC, for the console's file picker.

/// The drive roots Windows reports, e.g. `C:\`.
pub fn drives() -> Vec<String> {
    // SAFETY: GetLogicalDrives takes no arguments and has no preconditions.
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| format!("{}:\\", char::from(b'A' + bit)))
        .collect()
}

/// Whether `path` is on a local drive this process sees, so that a file
/// missing there is missing for the signed-in user too. A host running as
/// SYSTEM sees neither the drive letters the user mapped or substituted
/// nor, usually, the network shares the user's own token can open.
pub fn local_volume(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    let letter = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
            _ => return false,
        },
        _ => return false,
    };
    let root: Vec<u16> = format!("{}:\\", char::from(letter))
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: The root path is NUL-terminated and outlives the call.
    let kind = unsafe {
        windows::Win32::Storage::FileSystem::GetDriveTypeW(windows::core::PCWSTR(root.as_ptr()))
    };
    // DRIVE_UNKNOWN, DRIVE_NO_ROOT_DIR and DRIVE_REMOTE.
    !matches!(kind, 0 | 1 | 4)
}

/// Deletes every file in `paths`, or none of them: each is opened for
/// delete, without following a link, and marked for deletion, and they go
/// together once all are marked. One a program holds (a DLL a game has
/// loaded, a file open without delete sharing) clears the marks already
/// set, so every file stays; the error names that file.
pub fn delete_all(
    paths: &[std::path::PathBuf],
) -> Result<(), (std::path::PathBuf, std::io::Error)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            DELETE, FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo, SetFileInformationByHandle,
        },
    };
    let mark = |file: &std::fs::File, delete: bool| {
        let info = FILE_DISPOSITION_INFO { DeleteFile: delete };
        // SAFETY: the handle is open for the call, and info is a live
        // FILE_DISPOSITION_INFO of the size passed.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                (&raw const info).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
        .map_err(|error| {
            let code = error.code().0 as u32;
            if code & 0xFFFF_0000 == 0x8007_0000 {
                std::io::Error::from_raw_os_error((code & 0xFFFF) as i32)
            } else {
                std::io::Error::other(error)
            }
        })
    };
    let mut marked = Vec::with_capacity(paths.len());
    for path in paths {
        let file = std::fs::OpenOptions::new()
            .access_mode(DELETE.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
            .and_then(|file| mark(&file, true).map(|()| file));
        match file {
            Ok(file) => marked.push(file),
            Err(error) => {
                for file in &marked {
                    let _ = mark(file, false);
                }
                return Err((path.clone(), error));
            }
        }
    }
    // Closing the handles deletes the files.
    drop(marked);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// A DLL a program has loaded stops the delete before any file goes,
    /// as does a file open without delete sharing; otherwise all go.
    #[test]
    fn files_are_deleted_all_or_none() {
        use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
        let folder = std::env::temp_dir().join(format!("rl-delete-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let system =
            std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let first = folder.join("a.dll");
        let loaded = folder.join("b.dll");
        std::fs::write(&first, b"first").unwrap();
        std::fs::copy(system.join("version.dll"), &loaded).unwrap();
        let wide: Vec<u16> = loaded.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: the path is NUL-terminated and outlives the call.
        let module = unsafe {
            windows::Win32::System::LibraryLoader::LoadLibraryW(windows::core::PCWSTR(
                wide.as_ptr(),
            ))
        }
        .unwrap();
        let (held, _) = super::delete_all(&[first.clone(), loaded.clone()]).unwrap_err();
        assert_eq!(held, loaded);
        assert!(first.is_file() && loaded.is_file());
        // SAFETY: the module was loaded above and is not used after.
        unsafe { windows::Win32::Foundation::FreeLibrary(module) }.unwrap();
        // Open without delete sharing.
        let open = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ
            .open(&loaded)
            .unwrap();
        let (held, error) = super::delete_all(&[first.clone(), loaded.clone()]).unwrap_err();
        assert_eq!((held, error.raw_os_error()), (loaded.clone(), Some(32)));
        assert!(first.is_file());
        drop(open);
        super::delete_all(&[first.clone(), loaded.clone()]).unwrap();
        assert!(!first.exists() && !loaded.exists());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn only_drives_this_process_sees_locally_are_local_volumes() {
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32\\cmd.exe");
        assert!(super::local_volume(&program));
        let verbatim = format!("\\\\?\\{}", program.display());
        assert!(super::local_volume(std::path::Path::new(&verbatim)));
        let drives = super::drives();
        let unused = (b'D'..=b'Z')
            .map(char::from)
            .find(|letter| !drives.contains(&format!("{letter}:\\")))
            .expect("a free drive letter");
        for path in [
            format!("{unused}:\\Games\\game.exe"),
            "\\\\rubylight-test.invalid\\games\\game.exe".into(),
            "relative\\game.exe".into(),
        ] {
            assert!(!super::local_volume(std::path::Path::new(&path)), "{path}");
        }
    }
}
