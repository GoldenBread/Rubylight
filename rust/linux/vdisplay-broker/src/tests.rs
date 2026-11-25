//! Tests on temp directories: a fake configfs, a fake sysfs that follows it, socket pairs.
//!
//! The expected requests and replies are those of libvirtualdisplay's `vibeshine-vkms` broker
//! (Nonary/libvirtualdisplay, MIT, Copyright (c) 2026 Chase Payne).
use crate::paths::Paths;
use crate::peer::{self, Peer};
use crate::pool;
use crate::protocol::{Ctx, authorized, handle};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

struct Env {
    root: PathBuf,
    paths: Paths,
    stop: Arc<AtomicBool>,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A tree with `n` DRM connectors Virtual-10.. (ids in a shuffled order of creation is not
/// possible: ids ascend), the configfs root with an empty `vkms`, and the run and sessions dirs.
fn fresh(n: usize) -> Env {
    static N: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "rubylight-broker-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let paths = Paths {
        configfs: root.join("config"),
        faux: root.join("faux"),
        run: root.join("run"),
        sessions: root.join("sessions"),
    };
    fs::create_dir_all(paths.configfs.join("vkms")).unwrap();
    fs::create_dir_all(&paths.sessions).unwrap();
    for i in 0..n {
        let dir = paths.drm().join(format!("card3/card3-Virtual-{}", 10 + i));
        fs::create_dir_all(&dir).unwrap();
        // Ids are not in name order, so the map must follow ids.
        fs::write(dir.join("connector_id"), format!("{}\n", 100 - i)).unwrap();
        fs::write(dir.join("status"), "disconnected\n").unwrap();
    }
    Env {
        root,
        paths,
        stop: Arc::new(AtomicBool::new(false)),
    }
}

/// The kernel's part: copy each configfs status (1 or 2) to its DRM status file.
fn follow_kernel(env: &Env) {
    let paths = env.paths.clone();
    let stop = env.stop.clone();
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Ok(map) = pool::Map::load(&paths) {
                for e in &map.connectors {
                    let conf = paths
                        .device()
                        .join("connectors")
                        .join(&e.slot)
                        .join("status");
                    let dir = crate::paths::drm_connector_dir(&paths, &e.drm).unwrap();
                    let text = match fs::read_to_string(conf).as_deref().map(str::trim) {
                        Ok("1") => "connected\n",
                        _ => "disconnected\n",
                    };
                    let temp = dir.join(".status.new");
                    let _ = fs::write(&temp, text);
                    let _ = fs::rename(&temp, dir.join("status"));
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

fn started(n: usize, follow: bool) -> (Env, Ctx) {
    let env = fresh(n);
    pool::start_with(&env.paths, n, Duration::from_millis(100)).unwrap();
    if follow {
        follow_kernel(&env);
    }
    let ctx = Ctx {
        paths: env.paths.clone(),
        group_gid: Some(4242),
        verify: Duration::from_millis(300),
    };
    (env, ctx)
}

fn user(uid: u32) -> Peer {
    Peer {
        pid: 1,
        uid,
        gid: uid,
        groups: vec![],
    }
}

fn seat_session(paths: &Paths, name: &str, body: &str) {
    fs::write(paths.sessions.join(name), body).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap().trim().to_string()
}

#[test]
fn pool_start_builds_chains_and_maps_by_connector_id() {
    let (env, _) = started(3, false);
    let device = env.paths.device();
    assert_eq!(read(&device.join("enabled")), "1");
    assert_eq!(read(&device.join("planes/primary-2/type")), "1");
    assert_eq!(read(&device.join("connectors/connector-3/status")), "2");
    assert!(
        device
            .join("connectors/connector-1/possible_encoders/encoder-1")
            .is_symlink()
    );
    assert!(
        device
            .join("encoders/encoder-2/possible_crtcs/crtc-2")
            .is_symlink()
    );
    assert!(
        device
            .join("planes/primary-1/possible_crtcs/crtc-1")
            .is_symlink()
    );
    // Lowest id (100 - 2) is Virtual-12, so connector-1 maps to it.
    let map = pool::Map::load(&env.paths).unwrap();
    let pairs: Vec<_> = map.connectors.iter().map(|e| (&*e.slot, &*e.drm)).collect();
    assert_eq!(
        pairs,
        [
            ("connector-1", "Virtual-12"),
            ("connector-2", "Virtual-11"),
            ("connector-3", "Virtual-10")
        ]
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(env.paths.pool_json()), 0o644);
    assert_eq!(mode(env.paths.run.clone()), 0o755);
    assert_eq!(mode(env.paths.leases()), 0o700);
}

#[test]
fn pool_start_twice_keeps_the_device_and_a_missing_connector_fails() {
    let (env, _) = started(2, false);
    fs::write(
        env.paths.device().join("connectors/connector-1/status"),
        "1\n",
    )
    .unwrap();
    pool::start_with(&env.paths, 2, Duration::from_millis(50)).unwrap();
    assert_eq!(
        read(&env.paths.device().join("connectors/connector-1/status")),
        "1"
    );
    assert!(pool::start_with(&env.paths, 0, Duration::ZERO).is_err());
    assert!(pool::start_with(&env.paths, 17, Duration::ZERO).is_err());
    let short = fresh(1);
    let error = pool::start_with(&short.paths, 2, Duration::from_millis(50)).unwrap_err();
    assert!(format!("{error:#}").contains("1 of 2"), "{error:#}");
    let none = fresh(0);
    fs::remove_dir_all(none.paths.configfs.join("vkms")).unwrap();
    assert!(pool::start_with(&none.paths, 1, Duration::ZERO).is_err());
}

#[test]
fn pool_stop_disconnects_everything_and_drops_leases() {
    let (env, ctx) = started(2, true);
    assert_eq!(
        handle(&ctx, &user(0), "connect Virtual-11"),
        "OK connected Virtual-11"
    );
    assert!(env.paths.leases().join("connector-1.owner").exists());
    pool::stop(&env.paths).unwrap();
    assert_eq!(
        read(&env.paths.device().join("connectors/connector-1/status")),
        "2"
    );
    assert!(!env.paths.leases().join("connector-1.owner").exists());
    assert!(env.paths.device().is_dir());
}

#[test]
fn authorization_covers_root_group_seat_user_and_nobody() {
    let (env, ctx) = started(1, false);
    assert!(authorized(&ctx, &user(0)));
    assert!(!authorized(&ctx, &user(1000)));
    let mut by_primary = user(1000);
    by_primary.gid = 4242;
    assert!(authorized(&ctx, &by_primary));
    let mut by_supplementary = user(1000);
    by_supplementary.groups = vec![7, 4242];
    assert!(authorized(&ctx, &by_supplementary));
    assert_eq!(
        handle(&ctx, &user(1000), "status Virtual-10"),
        "ERROR not allowed"
    );
    assert_eq!(handle(&ctx, &user(1000), "bogus"), "ERROR not allowed");
    let active = "UID=1000\nACTIVE=1\nREMOTE=0\nSEAT=seat0\n";
    seat_session(&env.paths, "2", active);
    assert!(authorized(&ctx, &user(1000)));
    // Another uid, an inactive session, a remote one, one without a seat: none count.
    seat_session(&env.paths, "3", "UID=1001\nACTIVE=0\nSEAT=seat0\n");
    seat_session(
        &env.paths,
        "4",
        "UID=1002\nACTIVE=1\nREMOTE=1\nSEAT=seat0\n",
    );
    seat_session(&env.paths, "5", "UID=1003\nACTIVE=1\n");
    for uid in [1001, 1002, 1003, 1004] {
        assert!(!authorized(&ctx, &user(uid)), "{uid}");
    }
    seat_session(&env.paths, "6", "UID=1005\nACTIVE=1\nSEAT=seat0\n");
    assert!(authorized(&ctx, &user(1005)));
}

fn alice_and_bob(env: &Env) {
    seat_session(&env.paths, "1", "UID=1000\nACTIVE=1\nSEAT=seat0\n");
    seat_session(&env.paths, "2", "UID=1001\nACTIVE=1\nSEAT=seat0\n");
}

#[test]
fn requests_get_the_replies_the_host_expects() {
    let (_env, ctx) = started(2, true);
    let root = user(0);
    let ask = |line: &str| handle(&ctx, &root, line);
    assert_eq!(ask("status Virtual-10"), "STATUS disconnected Virtual-10");
    assert_eq!(ask("connect Virtual-10"), "OK connected Virtual-10");
    assert_eq!(ask("status Virtual-10"), "STATUS connected Virtual-10");
    assert_eq!(ask("disconnect Virtual-10"), "OK disconnected Virtual-10");
    assert_eq!(ask("status Virtual-10"), "STATUS disconnected Virtual-10");
    assert_eq!(ask("disconnect Virtual-10"), "OK disconnected Virtual-10");
    for bad in [
        "connect DP-1",
        "connect Virtual-9",
        "connect Virtual-010",
        "connect ../x",
        "status Virtual-99",
    ] {
        assert_eq!(ask(bad), "ERROR unknown connector", "{bad}");
    }
    assert_eq!(
        ask("mode Virtual-10 1920 1080 60000"),
        "ERROR modes are set through the compositor"
    );
    for bad in [
        "",
        "connect",
        "connect  Virtual-10",
        "connect Virtual-10 x",
        "reboot",
        "CONNECT Virtual-10",
    ] {
        assert_eq!(ask(bad), "ERROR unknown request", "{bad:?}");
    }
}

#[test]
fn a_lease_keeps_other_users_away_and_root_overrides() {
    let (env, ctx) = started(1, true);
    alice_and_bob(&env);
    let (alice, bob, root) = (user(1000), user(1001), user(0));
    assert_eq!(
        handle(&ctx, &alice, "connect Virtual-10"),
        "OK connected Virtual-10"
    );
    assert_eq!(read(&env.paths.leases().join("connector-1.owner")), "1000");
    assert_eq!(
        handle(&ctx, &alice, "connect Virtual-10"),
        "OK connected Virtual-10"
    );
    assert_eq!(
        handle(&ctx, &bob, "connect Virtual-10"),
        "ERROR connector owned by another uid"
    );
    assert_eq!(
        handle(&ctx, &bob, "disconnect Virtual-10"),
        "ERROR connector owned by another uid"
    );
    assert_eq!(
        handle(&ctx, &bob, "status Virtual-10"),
        "STATUS connected Virtual-10"
    );
    assert_eq!(
        handle(&ctx, &root, "connect Virtual-10"),
        "OK connected Virtual-10"
    );
    assert_eq!(
        handle(&ctx, &root, "disconnect Virtual-10"),
        "OK disconnected Virtual-10"
    );
    assert!(!env.paths.leases().join("connector-1.owner").exists());
    assert_eq!(
        handle(&ctx, &bob, "connect Virtual-10"),
        "OK connected Virtual-10"
    );
    assert_eq!(
        handle(&ctx, &bob, "disconnect Virtual-10"),
        "OK disconnected Virtual-10"
    );
}

#[test]
fn a_connected_connector_without_a_lease_is_root_s_to_adopt() {
    let (env, ctx) = started(1, true);
    alice_and_bob(&env);
    fs::write(
        env.paths.device().join("connectors/connector-1/status"),
        "1\n",
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let alice = user(1000);
    assert_eq!(
        handle(&ctx, &alice, "connect Virtual-10"),
        "ERROR connector ownership unavailable"
    );
    assert_eq!(
        handle(&ctx, &alice, "disconnect Virtual-10"),
        "ERROR connector ownership unavailable"
    );
    assert_eq!(
        handle(&ctx, &user(0), "connect Virtual-10"),
        "OK connected Virtual-10"
    );
    assert_eq!(read(&env.paths.leases().join("connector-1.owner")), "0");
    assert_eq!(
        handle(&ctx, &alice, "disconnect Virtual-10"),
        "ERROR connector owned by another uid"
    );
}

#[test]
fn a_connector_that_does_not_show_up_is_rolled_back() {
    let (env, ctx) = started(1, false);
    alice_and_bob(&env);
    let reply = handle(&ctx, &user(1000), "connect Virtual-10");
    assert_eq!(reply, "ERROR connector did not connect");
    assert!(!env.paths.leases().join("connector-1.owner").exists());
    assert_eq!(
        read(&env.paths.device().join("connectors/connector-1/status")),
        "2"
    );
}

#[test]
fn unsafe_leases_are_refused() {
    let (env, ctx) = started(1, true);
    alice_and_bob(&env);
    let lease = env.paths.leases().join("connector-1.owner");
    // A symlink as a lease: nobody follows it, not even root.
    std::os::unix::fs::symlink("/etc/passwd", &lease).unwrap();
    assert_eq!(
        handle(&ctx, &user(1000), "connect Virtual-10"),
        "ERROR connector lease unavailable"
    );
    assert_eq!(
        handle(&ctx, &user(0), "connect Virtual-10"),
        "ERROR connector lease unavailable"
    );
    fs::remove_file(&lease).unwrap();
    assert_eq!(
        handle(&ctx, &user(0), "connect Virtual-10"),
        "OK connected Virtual-10"
    );
    assert_eq!(read(&lease), "0");
    // Garbage inside a lease.
    fs::write(&lease, "abc\n").unwrap();
    assert_eq!(
        handle(&ctx, &user(1000), "disconnect Virtual-10"),
        "ERROR connector lease unavailable"
    );
    // A lease directory that is a symlink.
    let other = env.root.join("elsewhere");
    fs::create_dir(&other).unwrap();
    fs::remove_dir_all(env.paths.leases()).unwrap();
    std::os::unix::fs::symlink(&other, env.paths.leases()).unwrap();
    assert_eq!(
        handle(&ctx, &user(1000), "connect Virtual-10"),
        "ERROR connector lease unavailable"
    );
}

#[test]
fn a_missing_pool_map_is_reported() {
    let (env, ctx) = started(1, false);
    fs::remove_file(env.paths.pool_json()).unwrap();
    assert_eq!(
        handle(&ctx, &user(0), "status Virtual-10"),
        "ERROR pool unavailable"
    );
}

#[test]
fn peer_credentials_and_one_bounded_line_over_a_socket_pair() {
    let (mut a, b) = UnixStream::pair().unwrap();
    let peer = Peer::of(b.as_raw_fd()).unwrap();
    // SAFETY: getuid and getgid take no arguments and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    assert_eq!((peer.uid, peer.gid), (uid, gid));
    assert!(peer.pid > 0);
    a.write_all(b"status Virtual-10\nrest").unwrap();
    let mut b = b;
    assert_eq!(
        peer::read_line(&mut b).as_deref(),
        Some("status Virtual-10")
    );
    // What follows the first line stays unread.
    let mut left = String::new();
    a.shutdown(std::net::Shutdown::Write).unwrap();
    b.read_to_string(&mut left).unwrap();
    assert_eq!(left, "rest");
}

#[test]
fn a_long_slow_or_odd_line_is_dropped() {
    let (mut a, mut b) = UnixStream::pair().unwrap();
    a.write_all(&[b'x'; 200]).unwrap();
    a.write_all(b"\n").unwrap();
    assert_eq!(peer::read_line(&mut b), None);
    let (mut a, mut b) = UnixStream::pair().unwrap();
    a.write_all(b"status\x01 Virtual-10\n").unwrap();
    assert_eq!(peer::read_line(&mut b), None);
    let (mut a, mut b) = UnixStream::pair().unwrap();
    let mut line = vec![b'y'; 126];
    line.push(b'\n');
    a.write_all(&line).unwrap();
    assert_eq!(peer::read_line(&mut b).map(|s| s.len()), Some(126));
    let (_a, mut b) = UnixStream::pair().unwrap();
    peer::set_read_timeout(b.as_raw_fd(), Duration::from_millis(50)).unwrap();
    assert_eq!(peer::read_line(&mut b), None);
    let (a, mut b) = UnixStream::pair().unwrap();
    drop(a);
    assert_eq!(peer::read_line(&mut b), None);
}

#[test]
fn flags_move_the_path_roots() {
    let mut args: Vec<String> = ["pool", "--run-dir", "/x/run", "start", "--faux", "/x/faux"]
        .map(String::from)
        .to_vec();
    let paths = Paths::take_from(&mut args).unwrap();
    assert_eq!(args, ["pool", "start"]);
    assert_eq!(paths.run, Path::new("/x/run"));
    assert_eq!(paths.faux, Path::new("/x/faux"));
    assert_eq!(paths.configfs, Path::new("/sys/kernel/config"));
    assert!(Paths::take_from(&mut vec!["--configfs".to_string()]).is_err());
}
