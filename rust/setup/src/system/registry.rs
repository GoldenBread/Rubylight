//! Windows registry read, enumeration and write helpers for Rubylight setup.
use super::wide;
use anyhow::{Result, bail};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::*,
    },
    core::{PCWSTR, PWSTR},
};

/// A string value (REG_SZ or REG_EXPAND_SZ, unexpanded) in the 64-bit view.
pub fn registry_string(root: HKEY, path: &str, value: &str) -> Option<String> {
    registry_string_view(root, path, value, KEY_WOW64_64KEY)
}
pub fn registry_string_view(
    root: HKEY,
    path: &str,
    value: &str,
    view: REG_SAM_FLAGS,
) -> Option<String> {
    let key = open_key(root, path, KEY_READ | view)?;
    let name = wide(value);
    let mut size = 0u32;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: `key` is an open key, `name` is NUL-terminated and the second read gets a buffer of `bytes` bytes.
    unsafe {
        if RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        ) != ERROR_SUCCESS
            || !matches!(kind, REG_SZ | REG_EXPAND_SZ)
            || size > 1 << 20
        {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buffer.len() * 2) as u32;
        if RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        buffer.truncate((bytes as usize) / 2);
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}
pub fn registry_dword(root: HKEY, path: &str, value: &str, view: REG_SAM_FLAGS) -> Option<u32> {
    let key = open_key(root, path, KEY_READ | view)?;
    let name = wide(value);
    let mut data = 0u32;
    let mut size = 4u32;
    let mut kind = REG_VALUE_TYPE::default();
    // SAFETY: `key` is an open key, `name` is NUL-terminated and `data` holds the four bytes `size` names.
    unsafe {
        (RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        ) == ERROR_SUCCESS
            && kind == REG_DWORD)
            .then_some(data)
    }
}
/// A DWORD under HKLM (64-bit view): None when the key or the value does not
/// exist, Err when it cannot be read or is not a DWORD.
pub fn machine_dword(path: &str, value: &str) -> Result<Option<u32>> {
    let (key, name) = (wide(path), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    // SAFETY: `key` and `name` are NUL-terminated and `data` holds the four bytes `size` names.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    match status {
        ERROR_SUCCESS => Ok(Some(data)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        other => bail!("cannot read HKLM\\{path}\\{value} ({})", other.0),
    }
}
pub struct Key(pub HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key came from RegOpenKeyExW or RegCreateKeyExW and is closed only here.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
pub fn open_key(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> Option<Key> {
    let path = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `path` is NUL-terminated UTF-16 and `key` is a valid out-pointer.
    unsafe {
        (RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, access, &mut key) == ERROR_SUCCESS)
            .then_some(Key(key))
    }
}
pub fn subkeys(root: HKEY, path: &str, view: REG_SAM_FLAGS) -> Vec<String> {
    let Some(key) = open_key(root, path, KEY_READ | view) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        // SAFETY: `key` is an open key and `length` is the capacity of `name` in characters.
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&name[..length as usize]));
    }
    names
}
/// Value names and string data of a key's values.
pub fn values(root: HKEY, path: &str, view: REG_SAM_FLAGS) -> Vec<String> {
    let Some(key) = open_key(root, path, KEY_READ | view) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 2048];
        let mut length = name.len() as u32;
        // SAFETY: `key` is an open key and `length` is the capacity of `name` in characters.
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&name[..length as usize]));
    }
    names
}
pub fn delete_value(root: HKEY, path: &str, value: &str, view: REG_SAM_FLAGS) {
    if let Some(key) = open_key(root, path, KEY_SET_VALUE | view) {
        let value = wide(value);
        // SAFETY: `key` is an open key and `value` is NUL-terminated UTF-16.
        unsafe {
            let _ = RegDeleteValueW(key.0, PCWSTR(value.as_ptr()));
        }
    }
}
pub fn delete_key(root: HKEY, path: &str) {
    let path = wide(path);
    // SAFETY: `root` is a predefined key and `path` is NUL-terminated UTF-16.
    unsafe {
        let _ = RegDeleteTreeW(root, PCWSTR(path.as_ptr()));
        let _ = RegDeleteKeyExW(root, PCWSTR(path.as_ptr()), KEY_WOW64_64KEY.0, None);
    }
}
pub enum Value<'a> {
    Text(&'a str),
    Number(u32),
}
pub fn write_key(root: HKEY, path: &str, values: &[(&str, Value)]) -> Result<()> {
    let path = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `path` is NUL-terminated UTF-16, `key` is a valid out-pointer and each value buffer outlives its write.
    unsafe {
        let status = RegCreateKeyExW(
            root,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE | KEY_WOW64_64KEY,
            None,
            &mut key,
            None,
        );
        if status != ERROR_SUCCESS {
            bail!("cannot create registry key ({})", status.0);
        }
        let key = Key(key);
        for (name, value) in values {
            let name = wide(name);
            let status = match value {
                Value::Text(text) => {
                    let data = wide(text);
                    RegSetValueExW(
                        key.0,
                        PCWSTR(name.as_ptr()),
                        None,
                        REG_SZ,
                        Some(std::slice::from_raw_parts(
                            data.as_ptr().cast(),
                            data.len() * 2,
                        )),
                    )
                }
                Value::Number(number) => RegSetValueExW(
                    key.0,
                    PCWSTR(name.as_ptr()),
                    None,
                    REG_DWORD,
                    Some(&number.to_le_bytes()),
                ),
            };
            if status != ERROR_SUCCESS {
                bail!("cannot write registry value ({})", status.0);
            }
        }
    }
    Ok(())
}
