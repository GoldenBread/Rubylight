//! Thread-priority inspection, CPU load spinners, RNG, and stats helpers for `input_path_probe`.

use butterpollo_win_graphics::capture::Priority;
use std::time::{Duration, Instant};
use windows::Win32::{Foundation::HANDLE, System::Threading::*};

#[repr(C)]
#[derive(Default)]
struct ThreadBasicInformation {
    exit_status: i32,
    teb: usize,
    client_id: [usize; 2],
    affinity: usize,
    priority: i32,
    base_priority: i32,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationThread(
        thread: HANDLE,
        class: u32,
        info: *mut core::ffi::c_void,
        len: u32,
        ret: *mut u32,
    ) -> i32;
}
fn thread_priority() -> (i32, i32, i32) {
    let mut info = ThreadBasicInformation::default();
    let mut ret = 0;
    unsafe {
        NtQueryInformationThread(
            GetCurrentThread(),
            0,
            (&mut info as *mut ThreadBasicInformation).cast(),
            size_of::<ThreadBasicInformation>() as u32,
            &mut ret,
        );
        (
            info.priority,
            info.base_priority,
            GetThreadPriority(GetCurrentThread()),
        )
    }
}

pub(crate) fn stats(name: &str, mut v: Vec<f64>) {
    if v.is_empty() {
        println!("{name:52} no samples");
        return;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    println!(
        "{name:52} n={n:5} mean {mean:8.1} p50 {:8.1} p90 {:8.1} p99 {:8.1} max {:9.1} us",
        v[n / 2],
        v[n * 90 / 100],
        v[n * 99 / 100],
        v[n - 1]
    );
}

pub(crate) struct Rng(pub(crate) u64);
impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Prio {
    /// No priority change.
    Plain,
    /// butterpollo_win_graphics::capture::Priority::new (the control loop's before
    /// October 7; capture, encode, audio and pacing threads still use it).
    PriorityNew,
    /// butterpollo_win_graphics::capture::Priority::input (the control loop's now).
    PriorityInput,
    /// MMCSS "Games"/HIGH without the SetThreadPriority that follows it.
    MmcssOnly,
}

pub(crate) struct MmcssOnly(HANDLE);
impl MmcssOnly {
    pub(crate) fn new() -> Self {
        unsafe {
            let name: Vec<u16> = "Games\0".encode_utf16().collect();
            let mut index = 0;
            let handle =
                AvSetMmThreadCharacteristicsW(windows::core::PCWSTR(name.as_ptr()), &mut index)
                    .unwrap_or_default();
            let _ = AvSetMmThreadPriority(handle, AVRT_PRIORITY_HIGH);
            Self(handle)
        }
    }
}
impl Drop for MmcssOnly {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

#[allow(dead_code)]
pub(crate) enum PrioGuard {
    A(Priority),
    B(MmcssOnly),
    None,
}

pub(crate) fn apply_prio(p: Prio) -> PrioGuard {
    match p {
        Prio::Plain => PrioGuard::None,
        Prio::PriorityNew => PrioGuard::A(Priority::new()),
        Prio::PriorityInput => PrioGuard::A(Priority::input()),
        Prio::MmcssOnly => PrioGuard::B(MmcssOnly::new()),
    }
}

/// A child process (normal priority class) spinning on every logical CPU at
/// the given relative thread priority, standing in for a CPU-bound game.
pub(crate) struct Load(Option<std::process::Child>);
impl Load {
    pub(crate) fn start(kind: &str) -> Self {
        if kind == "none" {
            return Self(None);
        }
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(format!("--spin={kind}"))
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        Self(Some(child))
    }
}
impl Drop for Load {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub(crate) fn spin_child(kind: &str) {
    let priority = match kind {
        "highest" => THREAD_PRIORITY_HIGHEST,
        "critical" => THREAD_PRIORITY_TIME_CRITICAL,
        _ => THREAD_PRIORITY_NORMAL,
    };
    let n = std::thread::available_parallelism().map_or(16, |n| n.get());
    let deadline = Instant::now() + Duration::from_secs(60);
    let threads: Vec<_> = (0..n)
        .map(|_| {
            std::thread::spawn(move || {
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), priority);
                }
                let mut x = 0u64;
                while Instant::now() < deadline {
                    for _ in 0..10_000 {
                        x = std::hint::black_box(
                            x.wrapping_mul(6364136223846793005).wrapping_add(1),
                        );
                    }
                }
            })
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
}

pub(crate) fn priority_section() {
    println!("== thread priority (NtQueryInformationThread: current, base; GetThreadPriority) ==");
    for prio in [
        Prio::Plain,
        Prio::PriorityNew,
        Prio::PriorityInput,
        Prio::MmcssOnly,
    ] {
        let h = std::thread::spawn(move || {
            let _p = apply_prio(prio);
            std::thread::sleep(Duration::from_millis(20));
            let mut seen = std::collections::BTreeMap::new();
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(1000) {
                *seen.entry(thread_priority()).or_insert(0u32) += 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            println!("{prio:?}: (current, base, GetThreadPriority) -> samples {seen:?}");
        });
        h.join().unwrap();
    }
}
