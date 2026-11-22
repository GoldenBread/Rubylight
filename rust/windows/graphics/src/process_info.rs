//! Local Win32 process-identity, desktop, and build-number queries for graphics helpers.
use anyhow::{Context, Result};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::{
    Win32::{
        Foundation::*,
        Security::*,
        System::{StationsAndDesktops::*, Threading::*},
    },
    core::PWSTR,
};

fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: callers pass a freshly created handle they own and that nothing else closes.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}

fn current_token() -> Result<OwnedHandle> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is a valid out-pointer and the handle it receives is owned by the caller.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
    }
    Ok(owned(token))
}

pub(crate) fn is_system() -> bool {
    let Ok(token) = current_token() else {
        return false;
    };
    let mut buffer = [0usize; 64];
    let mut needed = 0;
    // SAFETY: `buffer` is usize-aligned and writable for the size passed.
    unsafe {
        if GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )
        .is_err()
        {
            return false;
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        IsWellKnownSid(user.User.Sid, WinLocalSystemSid).as_bool()
    }
}

pub(crate) fn own_sid() -> Result<String> {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    let token = current_token()?;
    let mut buffer = [0usize; 64];
    let mut needed = 0;
    // SAFETY: `buffer` is usize-aligned and writable for the size passed; `text` is freed with LocalFree.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid.context("Windows returned an invalid SID")
    }
}

struct InputDesktop {
    original: HDESK,
    current: HDESK,
}
impl Drop for InputDesktop {
    fn drop(&mut self) {
        // SAFETY: The original desktop handle is borrowed from Windows; the owned current handle is closed after detaching.
        unsafe {
            let _ = SetThreadDesktop(self.original);
            let _ = CloseDesktop(self.current);
        }
    }
}
thread_local! {
    static INPUT_DESKTOP: std::cell::RefCell<Option<InputDesktop>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn follow_input_desktop() -> bool {
    // SAFETY: The thread-local attachment owns every opened desktop handle.
    INPUT_DESKTOP.with_borrow_mut(|attachment| unsafe {
        let Ok(desktop) = OpenInputDesktop(
            DF_ALLOWOTHERACCOUNTHOOK,
            false,
            DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0),
        ) else {
            return false;
        };
        let Ok(original) = GetThreadDesktop(GetCurrentThreadId()) else {
            let _ = CloseDesktop(desktop);
            return false;
        };
        if SetThreadDesktop(desktop).is_err() {
            let _ = CloseDesktop(desktop);
            return false;
        }
        if let Some(attached) = attachment {
            let previous = std::mem::replace(&mut attached.current, desktop);
            let _ = CloseDesktop(previous);
        } else {
            *attachment = Some(InputDesktop {
                original,
                current: desktop,
            });
        }
        true
    })
}

#[cfg(test)]
pub(crate) fn current_session() -> Result<u32> {
    let mut session = 0u32;
    // SAFETY: `session` is a live out-pointer and GetCurrentProcessId has no preconditions.
    unsafe {
        windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(
            GetCurrentProcessId(),
            &mut session,
        )?;
    }
    Ok(session)
}

pub(crate) fn windows_build() -> Option<u32> {
    static BUILD: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *BUILD
        // SAFETY: The registry names are terminated and the output size matches the live UTF-16 buffer.
        .get_or_init(|| unsafe {
            use windows::Win32::System::Registry::*;
            let mut text = [0u16; 32];
            let mut size = (text.len() * 2) as u32;
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                windows::core::w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
                windows::core::w!("CurrentBuildNumber"),
                RRF_RT_REG_SZ,
                None,
                Some(text.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            crate::text::from_wide(&text).trim().parse().ok()
        })
}
