//! What a background writer such as Stream DVR's clip writer needs from Linux: the free space
//! on a folder's volume, and a lower priority for its own thread so it never competes with the
//! stream's threads.
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// How far below the stream's threads a background writer runs, in nice steps: Windows'
/// below-normal priority, one class under normal.
const LOWER_BY: libc::c_int = 5;

/// The bytes the host may still write to `folder`'s volume (what an unprivileged user may use).
#[allow(
    clippy::useless_conversion,
    reason = "narrower C types on 32-bit Linux"
)]
pub fn free_bytes(folder: &Path) -> io::Result<u64> {
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: an all-zero `statvfs` is a valid value the call overwrites.
    let mut found: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `found` is a live local.
    if unsafe { libc::statvfs(path.as_ptr(), &mut found) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from(found.f_bavail).saturating_mul(u64::from(found.f_frsize)))
}

/// Lowers the calling thread's priority (its nice value; Linux keeps one per thread); best
/// effort.
pub fn lower_current_thread() {
    // SAFETY: gettid takes no arguments and cannot fail.
    let thread = unsafe { libc::gettid() } as libc::id_t;
    // SAFETY: the calls take no pointers; errno is cleared first, as -1 is also a valid nice.
    unsafe {
        *libc::__errno_location() = 0;
        let now = libc::getpriority(libc::PRIO_PROCESS, thread);
        if now == -1 && *libc::__errno_location() != 0 {
            return;
        }
        let _ = libc::setpriority(libc::PRIO_PROCESS, thread, (now + LOWER_BY).min(19));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_temp_volume_has_free_space() {
        assert!(super::free_bytes(&std::env::temp_dir()).unwrap() > 0);
        assert!(super::free_bytes(std::path::Path::new("/no/such/folder")).is_err());
        std::thread::spawn(super::lower_current_thread)
            .join()
            .unwrap();
    }
}
