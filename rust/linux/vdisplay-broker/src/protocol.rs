//! One request, one reply: who may ask, what each request does, how leases decide.
//!
//! The requests, replies and lease rules follow libvirtualdisplay's `linux/packaging/
//! vibeshine-vkms` broker (Nonary/libvirtualdisplay, MIT, Copyright (c) 2026 Chase Payne),
//! which Vibepollo installs; the monitors are the pool's VKMS connectors.
use crate::lease::Leases;
use crate::paths::{self, Paths};
use crate::peer::{self, Peer};
use crate::pool::{self, Map};
use std::fs::{self, File};
use std::io::Write;
use std::mem::ManuallyDrop;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};

/// The group whose members may plug monitors in without a session on a seat.
pub const GROUP: &str = "rubylight-vdisplay";

/// What a request needs: the paths, the group's gid and how long a plug may take to show.
pub struct Ctx {
    pub paths: Paths,
    pub group_gid: Option<u32>,
    pub verify: Duration,
}

impl Ctx {
    /// Production settings over `paths`.
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            group_gid: group_gid(GROUP),
            verify: Duration::from_secs(2),
        }
    }
}

/// The gid of the group `name`, if it exists.
fn group_gid(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut buffer = vec![0u8; 4096];
    // SAFETY: an all-zero `group` is a valid out-parameter; getgrnam_r fills it.
    let mut group: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call and the buffer length is passed along.
    let rc = unsafe {
        libc::getgrnam_r(
            name.as_ptr(),
            &mut group,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    (rc == 0 && !result.is_null()).then_some(group.gr_gid)
}

/// Whether `uid` has an active, local session on a seat in logind's state files.
fn seat_user(paths: &Paths, uid: u32) -> bool {
    let Ok(entries) = fs::read_dir(&paths.sessions) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') || !entry.path().is_file() {
            return false;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            return false;
        };
        let value = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        };
        value("UID").and_then(|v| v.parse::<u32>().ok()) == Some(uid)
            && value("SEAT").is_some_and(|s| !s.is_empty())
            && value("ACTIVE") == Some("1")
            && matches!(value("REMOTE"), None | Some("0"))
    })
}

/// Whether the caller may use the broker at all.
pub fn authorized(ctx: &Ctx, peer: &Peer) -> bool {
    peer.uid == 0
        || ctx
            .group_gid
            .is_some_and(|g| peer.gid == g || peer.groups.contains(&g))
        || seat_user(&ctx.paths, peer.uid)
}

/// The reply to one request line from `peer`.
pub fn handle(ctx: &Ctx, peer: &Peer, line: &str) -> String {
    if !authorized(ctx, peer) {
        return "ERROR not allowed".into();
    }
    let words: Vec<&str> = line.split(' ').collect();
    match words.as_slice() {
        ["mode", ..] => "ERROR modes are set through the compositor".into(),
        [verb @ ("connect" | "disconnect" | "status"), name] => {
            if !paths::public_name(name) {
                return "ERROR unknown connector".into();
            }
            let Ok(map) = Map::load(&ctx.paths) else {
                return "ERROR pool unavailable".into();
            };
            let Some(slot) = map.slot_of(name) else {
                return "ERROR unknown connector".into();
            };
            match *verb {
                "status" => status(ctx, name),
                action => match mutate(ctx, peer, action == "connect", slot, name) {
                    Ok(reply) | Err(reply) => reply,
                },
            }
        }
        _ => "ERROR unknown request".into(),
    }
}

/// The DRM status text of `name`: `connected`, `disconnected`, or what the kernel says.
fn drm_status(ctx: &Ctx, name: &str) -> Option<String> {
    let dir = paths::drm_connector_dir(&ctx.paths, name)?;
    Some(
        fs::read_to_string(dir.join("status"))
            .ok()?
            .trim()
            .to_string(),
    )
}

fn status(ctx: &Ctx, name: &str) -> String {
    match drm_status(ctx, name).as_deref() {
        Some("connected") => format!("STATUS connected {name}"),
        Some("disconnected") => format!("STATUS disconnected {name}"),
        _ => "ERROR invalid connector state".into(),
    }
}

/// Connect or disconnect `slot` for `peer` under its lease. Both arms are replies.
fn mutate(ctx: &Ctx, peer: &Peer, connect: bool, slot: &str, name: &str) -> Result<String, String> {
    let unavailable = |what: &'static str| {
        move |e: anyhow::Error| {
            eprintln!("{what}: {e:#}");
            format!("ERROR {what}")
        }
    };
    let leases =
        Leases::open(&ctx.paths.leases()).map_err(unavailable("connector lease unavailable"))?;
    let _lock = leases
        .lock(slot)
        .map_err(unavailable("connector lease unavailable"))?;
    let connected = match drm_status(ctx, name).as_deref() {
        Some("connected") => true,
        Some("disconnected") => false,
        _ => return Err("ERROR invalid connector state".into()),
    };
    let root = peer.uid == 0;
    let owner = match leases.read(slot) {
        Ok(owner) => owner,
        // Only root clears a lease it cannot read.
        Err(_) if root && leases.release(slot).is_ok() => None,
        Err(e) => return Err(unavailable("connector lease unavailable")(e)),
    };
    let other = |owner: u32| !root && owner != peer.uid;
    if connect {
        if connected {
            match owner {
                None if !root => return Err("ERROR connector ownership unavailable".into()),
                None => leases
                    .write(slot, 0)
                    .map_err(unavailable("connector ownership unavailable"))?,
                Some(owner) if other(owner) => {
                    return Err("ERROR connector owned by another uid".into());
                }
                Some(_) => {}
            }
            return Ok(format!("OK connected {name}"));
        }
        // A disconnected connector is not in use: a lease left by an interrupted disconnect goes.
        leases
            .release(slot)
            .and_then(|()| leases.write(slot, peer.uid))
            .map_err(unavailable("connector lease unavailable"))?;
        if let Err(e) = pool::set_status(&ctx.paths, slot, true) {
            eprintln!("{e:#}");
            let _ = leases.release(slot);
            return Err("ERROR connector update failed".into());
        }
        if !wait_for(ctx, name, "connected") {
            let _ = pool::set_status(&ctx.paths, slot, false);
            let _ = leases.release(slot);
            return Err("ERROR connector did not connect".into());
        }
        Ok(format!("OK connected {name}"))
    } else {
        match owner {
            Some(owner) if other(owner) => {
                return Err("ERROR connector owned by another uid".into());
            }
            None if connected && !root => {
                return Err("ERROR connector ownership unavailable".into());
            }
            _ => {}
        }
        if connected {
            pool::set_status(&ctx.paths, slot, false).map_err(|e| {
                eprintln!("{e:#}");
                "ERROR connector update failed".to_string()
            })?;
            if !wait_for(ctx, name, "disconnected") {
                return Err("ERROR connector update failed".into());
            }
        }
        leases
            .release(slot)
            .map_err(unavailable("connector lease cleanup failed"))?;
        Ok(format!("OK disconnected {name}"))
    }
}

/// Poll until the DRM status of `name` reads `want`, for at most `ctx.verify`.
fn wait_for(ctx: &Ctx, name: &str, want: &str) -> bool {
    let deadline = Instant::now() + ctx.verify;
    loop {
        if drm_status(ctx, name).as_deref() == Some(want) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `serve`: the connection is stdin (read) and stdout (write).
pub fn serve_stdio(paths: &Paths) -> anyhow::Result<()> {
    // SAFETY: descriptors 0 and 1 are the connection systemd passes; ManuallyDrop keeps them
    // open for the process's exit.
    let (mut input, mut output) = unsafe {
        (
            ManuallyDrop::new(File::from_raw_fd(0)),
            ManuallyDrop::new(File::from_raw_fd(1)),
        )
    };
    let peer = Peer::of(input.as_raw_fd())?;
    peer::set_read_timeout(input.as_raw_fd(), peer::READ_TIMEOUT)?;
    let ctx = Ctx::new(paths.clone());
    let line = peer::read_line(&mut *input);
    let reply = match &line {
        Some(line) => handle(&ctx, &peer, line),
        None => "ERROR unknown request".into(),
    };
    eprintln!(
        "uid={} pid={} request={:?} -> {reply}",
        peer.uid,
        peer.pid,
        line.as_deref().unwrap_or("<none>")
    );
    output.write_all(format!("{reply}\n").as_bytes())?;
    Ok(())
}
