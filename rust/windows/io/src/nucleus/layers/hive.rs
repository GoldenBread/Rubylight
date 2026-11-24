//! Windows registry hive access (`HKEY_USERS` and `HKEY_LOCAL_MACHINE`) for compatibility layers.

use super::*;

/// The machine's compatibility layers (`HKEY_LOCAL_MACHINE`), which apply
/// to every user; none without the key.
pub fn machine_values() -> Result<BTreeMap<String, Value>> {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let path = to_wide(KEY);
    let mut key = HKEY::default();
    // SAFETY: the path is terminated and outlives the call; `key` is an
    // out-pointer, closed by `Key` once opened.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            Some(0),
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
        return Ok(BTreeMap::new());
    }
    opened
        .ok()
        .with_context(|| format!(r"opening HKEY_LOCAL_MACHINE\{KEY}"))?;
    read_values(&Key(key)).with_context(|| format!(r"reading HKEY_LOCAL_MACHINE\{KEY}"))
}

/// A user's layers in their hive (`HKEY_USERS\<SID>`), which Nucleus Co-op,
/// running as them, sees as its own.
pub struct UserHive {
    sid: String,
    path: Vec<u16>,
}

impl UserHive {
    /// The user the host, in their session, runs Nucleus Co-op as.
    pub fn signed_in() -> Result<Self> {
        let sid = crate::process::user_sid().context(
            "finding the signed-in user, whose compatibility settings Nucleus Co-op changes",
        )?;
        Ok(Self::of(sid))
    }

    /// The user `sid`'s.
    pub fn of(sid: String) -> Self {
        let path = to_wide(&format!(r"{sid}\{KEY}"));
        Self { sid, path }
    }

    /// Whether the user's hive is loaded, as it is while they are signed in.
    /// Without it, their layers can't be read: none is not "no values".
    fn loaded(&self) -> Result<()> {
        let root = to_wide(&self.sid);
        let mut key = HKEY::default();
        // SAFETY: the SID is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once opened.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                PCWSTR(root.as_ptr()),
                Some(0),
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
            anyhow::bail!(
                r"HKEY_USERS\{} is not loaded: the user is not signed in, so their compatibility settings wait until they are",
                self.sid
            );
        }
        opened
            .ok()
            .with_context(|| format!(r"opening HKEY_USERS\{}", self.sid))?;
        drop(Key(key));
        Ok(())
    }

    /// The key opened with `access`; None when it is not there in the
    /// user's loaded hive.
    fn open(&self, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<Option<Key>> {
        let mut key = HKEY::default();
        // SAFETY: the path is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once opened.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                PCWSTR(self.path.as_ptr()),
                Some(0),
                access,
                &mut key,
            )
        };
        if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
            self.loaded()?;
            return Ok(None);
        }
        opened.ok().with_context(|| self.describe("opening"))?;
        Ok(Some(Key(key)))
    }

    fn describe(&self, what: &str) -> String {
        format!(r"{what} HKEY_USERS\{}\{KEY}", self.sid)
    }
}

/// An open registry key, closed on drop.
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

impl Layers for UserHive {
    fn user(&self) -> &str {
        &self.sid
    }

    fn values(&mut self) -> Result<BTreeMap<String, Value>> {
        let Some(key) = self.open(KEY_QUERY_VALUE)? else {
            return Ok(BTreeMap::new());
        };
        read_values(&key).with_context(|| self.describe("reading"))
    }

    fn set(&mut self, name: &str, value: &Value) -> Result<()> {
        let mut key = HKEY::default();
        // SAFETY: the path is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once created.
        unsafe {
            RegCreateKeyExW(
                HKEY_USERS,
                PCWSTR(self.path.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        }
        .ok()
        .with_context(|| self.describe("opening"))?;
        let key = Key(key);
        let wide = to_wide(name);
        // SAFETY: the name is terminated and the data a live slice.
        unsafe {
            RegSetValueExW(
                key.0,
                PCWSTR(wide.as_ptr()),
                None,
                REG_VALUE_TYPE(value.kind),
                Some(&value.data),
            )
        }
        .ok()
        .with_context(|| self.describe(&format!("setting {name} in")))
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        let Some(key) = self.open(KEY_SET_VALUE)? else {
            return Ok(());
        };
        let wide = to_wide(name);
        // SAFETY: the key is open and the name terminated.
        let deleted = unsafe { RegDeleteValueW(key.0, PCWSTR(wide.as_ptr())) };
        if deleted == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        deleted
            .ok()
            .with_context(|| self.describe(&format!("removing {name} from")))
    }
}

/// Every value of `key`. A value that grew while it was read is read again
/// with room for it.
fn read_values(key: &Key) -> Result<BTreeMap<String, Value>> {
    let (mut name_chars, mut data_bytes) = (0u32, 0u32);
    // SAFETY: the key is open; the counts are out-pointers.
    unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut name_chars),
            Some(&mut data_bytes),
            None,
            None,
        )
    }
    .ok()?;
    let mut name = vec![0u16; name_chars as usize + 1];
    let mut data = vec![0u8; (data_bytes as usize).max(1)];
    let mut values = BTreeMap::new();
    let mut index = 0;
    let mut retries = 0;
    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: the key is open, and the buffers and their lengths are
        // live for the call.
        let result = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
            return Ok(values);
        }
        if result == ERROR_MORE_DATA && retries < 8 {
            retries += 1;
            name.resize(name.len() * 2, 0);
            data.resize((data_len as usize).max(data.len() * 2), 0);
            continue;
        }
        result.ok()?;
        values.insert(
            String::from_utf16_lossy(&name[..name_len as usize]),
            Value {
                kind,
                data: data[..data_len as usize].to_vec(),
            },
        );
        index += 1;
        retries = 0;
    }
}
