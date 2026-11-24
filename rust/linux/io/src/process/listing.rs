//! The running processes as `/proc` shows them: the list, a process's program, start time and usage, and the tree a started command grew. Nothing here is unavailable on Linux.

use anyhow::{Context, Result};
use std::{
    collections::BTreeSet,
    sync::OnceLock,
    time::Duration,
};

/// The clock's units in a second: process times are in 100 ns units, as the host keeps them.
const UNITS_PER_SECOND: u128 = 10_000_000;
/// The bits of a time below the boot's tag: 2^52 units are about 14 years of uptime.
const TAG_SHIFT: u32 = 52;

/// One line of `/proc/<pid>/stat`, as far as the host reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stat {
    pub(super) pid: u32,
    /// The kernel's name of the program, at most 15 bytes.
    pub(super) name: String,
    /// `R`, `S`, ...; `Z` and `X` are processes that have exited.
    pub(super) state: char,
    pub(super) parent: u32,
    pub(super) group: u32,
    pub(super) session: u32,
    /// User and kernel CPU time, in clock ticks.
    pub(super) cpu_ticks: u64,
    /// When it started, in clock ticks since the boot.
    pub(super) start_ticks: u64,
    pub(super) resident_pages: u64,
}
impl Stat {
    /// Whether the process still runs: one that has exited stays listed until its parent
    /// has read its exit code.
    pub(super) fn alive(&self) -> bool {
        !matches!(self.state, 'Z' | 'X')
    }
}

/// A `stat` line read into its fields. The program's name stands in parentheses and may
/// hold spaces and parentheses itself, so the fields after it are counted from the last
/// `)`.
pub(super) fn parse_stat(text: &str) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = text.get(..open)?.trim().parse().ok()?;
    let name = text.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = text.get(close + 1..)?.split_ascii_whitespace().collect();
    let number = |index: usize| -> Option<u64> { fields.get(index)?.parse().ok() };
    Some(Stat {
        pid,
        name,
        state: fields.first()?.chars().next()?,
        parent: u32::try_from(number(1)?).ok()?,
        group: u32::try_from(number(2)?).ok()?,
        session: u32::try_from(number(3)?).ok()?,
        cpu_ticks: number(11)?.checked_add(number(12)?)?,
        start_ticks: number(19)?,
        // A kernel thread reports 0 pages; the field is signed in old kernels.
        resident_pages: number(21).unwrap_or(0),
    })
}

pub(super) fn read_stat(pid: u32) -> Option<Stat> {
    parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Every process `/proc` lists, exited ones included.
fn all_stats() -> Result<Vec<Stat>> {
    let mut stats = Vec::new();
    for entry in std::fs::read_dir("/proc").context("listing /proc")? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        // A process that ended since the listing has no stat any more.
        if let Some(stat) = read_stat(pid) {
            stats.push(stat);
        }
    }
    Ok(stats)
}

/// The kernel's clock ticks in a second (`_SC_CLK_TCK`), 100 where it cannot be asked.
fn ticks_per_second() -> u64 {
    static TICKS: OnceLock<u64> = OnceLock::new();
    *TICKS.get_or_init(|| {
        // SAFETY: sysconf takes a name and no pointers.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        u64::try_from(ticks).ok().filter(|&t| t > 0).unwrap_or(100)
    })
}

fn page_bytes() -> u64 {
    static BYTES: OnceLock<u64> = OnceLock::new();
    *BYTES.get_or_init(|| {
        // SAFETY: sysconf takes a name and no pointers.
        let bytes = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        u64::try_from(bytes).ok().filter(|&b| b > 0).unwrap_or(4096)
    })
}

/// Clock ticks as 100 ns units.
fn units(ticks: u64) -> u64 {
    let units = u128::from(ticks) * UNITS_PER_SECOND / u128::from(ticks_per_second());
    u64::try_from(units).unwrap_or(u64::MAX)
}

/// What tells this boot's times from another boot's: a number from the kernel's boot ID
/// in the bits above the time, so a start time kept from before a restart of the PC never
/// equals one read after it. 0 when the boot ID cannot be read.
fn boot_tag() -> u64 {
    static TAG: OnceLock<u64> = OnceLock::new();
    *TAG.get_or_init(|| {
        let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
        tag_of(id.trim())
    })
}

/// The tag of the boot ID `id`: 11 bits of its FNV-1a hash, above [`TAG_SHIFT`].
fn tag_of(id: &str) -> u64 {
    if id.is_empty() {
        return 0;
    }
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    (hash & 0x7ff) << TAG_SHIFT
}

/// A start in clock ticks since the boot as the host's time: 100 ns units since the boot,
/// on the clock that keeps counting while the PC sleeps, under [`boot_tag`].
pub(super) fn started(start_ticks: u64) -> u64 {
    boot_tag() + units(start_ticks).min((1 << TAG_SHIFT) - 1)
}

/// The current time on the clock of [`creation_time`], rounded down to the kernel's clock
/// tick as process start times are: a process started after this call never reads as
/// started before it.
pub fn now() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a live local the call writes once.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
        return 0;
    }
    let since_boot = Duration::new(
        u64::try_from(time.tv_sec).unwrap_or(0),
        u32::try_from(time.tv_nsec).unwrap_or(0),
    );
    let ticks = since_boot.as_nanos() * u128::from(ticks_per_second()) / 1_000_000_000;
    started(u64::try_from(ticks).unwrap_or(u64::MAX))
}

/// A process's start time (100 ns units since the boot, see [`now`]), which tells a
/// reused process ID apart; 0 when it cannot be read or the process has exited.
pub fn creation_time(pid: u32) -> u64 {
    match read_stat(pid) {
        Some(stat) if stat.alive() => started(stat.start_ticks),
        _ => 0,
    }
}

/// A process's sampled CPU and resident memory usage.
pub struct Usage {
    pub started: u64,
    /// Kernel and user CPU time in 100 ns units.
    pub cpu_time: u64,
    /// Resident memory in bytes.
    pub working_set: u64,
}

/// CPU time and resident memory, with the start time to reject reused IDs.
pub fn usage(pid: u32) -> Option<Usage> {
    let stat = read_stat(pid).filter(Stat::alive)?;
    Some(Usage {
        started: started(stat.start_ticks),
        cpu_time: units(stat.cpu_ticks),
        working_set: stat.resident_pages.saturating_mul(page_bytes()),
    })
}

/// The last name of a program path, whichever way its separators lean: a Windows program
/// under Wine or Proton keeps its Windows path.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A program's name as the host matches it. The kernel's name is cut at 15 bytes, so one
/// that long is completed from the name the program was started under, when that begins
/// with it.
fn full_name(stat: &Stat) -> String {
    const KERNEL_NAME_BYTES: usize = 15;
    if stat.name.len() < KERNEL_NAME_BYTES {
        return stat.name.clone();
    }
    let started_as = std::fs::read(format!("/proc/{}/cmdline", stat.pid)).unwrap_or_default();
    let first = started_as.split(|byte| *byte == 0).next().unwrap_or(&[]);
    let first = String::from_utf8_lossy(first);
    let name = file_name(&first);
    if name.starts_with(&stat.name) {
        name.to_owned()
    } else {
        stat.name.clone()
    }
}

/// Every running process with its parent, start time and program name, from `/proc`. A
/// process that has exited and waits for its parent to read its exit code is left out.
pub fn processes() -> Result<Vec<butterpollo_session::steam::Process>> {
    Ok(all_stats()?
        .iter()
        .filter(|stat| stat.alive())
        .map(|stat| butterpollo_session::steam::Process {
            pid: stat.pid,
            parent: stat.parent,
            started: started(stat.start_ticks),
            name: full_name(stat),
        })
        .collect())
}

/// A process's full program path (`/proc/<pid>/exe`); None for a process of another user,
/// a kernel thread, or one that has ended. A Windows program under Wine or Proton reports
/// the loader that runs it.
pub fn image_path(pid: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let path = path.to_string_lossy();
    // The kernel marks a program whose file was replaced or removed since it started.
    Some(path.strip_suffix(" (deleted)").unwrap_or(&path).to_owned())
}

/// The running processes of the tree a command started as `root` grew, each with its
/// start in clock ticks: those in the session or process group `root` leads, and their
/// descendants. A program that left both and its parent (a daemon) is not found.
pub(super) fn tree(root: u32) -> Vec<(u32, u64)> {
    let stats: Vec<Stat> = all_stats()
        .unwrap_or_default()
        .into_iter()
        .filter(Stat::alive)
        .collect();
    tree_of(root, &stats)
}

/// [`tree`] in the processes `stats`.
pub(super) fn tree_of(root: u32, stats: &[Stat]) -> Vec<(u32, u64)> {
    let mut members: BTreeSet<u32> = stats
        .iter()
        .filter(|stat| stat.session == root || stat.group == root)
        .map(|stat| stat.pid)
        .collect();
    loop {
        let children: Vec<u32> = stats
            .iter()
            .filter(|stat| members.contains(&stat.parent) && !members.contains(&stat.pid))
            .map(|stat| stat.pid)
            .collect();
        if children.is_empty() {
            break;
        }
        members.extend(children);
    }
    stats
        .iter()
        .filter(|stat| members.contains(&stat.pid))
        .map(|stat| (stat.pid, stat.start_ticks))
        .collect()
}

/// Whether the process `pid` runs and started at `start_ticks`: the same process, not
/// another that took its ID.
pub(super) fn same_process(pid: u32, start_ticks: u64) -> bool {
    read_stat(pid).is_some_and(|stat| stat.alive() && stat.start_ticks == start_ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(pid: u32, parent: u32, group: u32, session: u32) -> Stat {
        Stat {
            pid,
            name: "x".into(),
            state: 'S',
            parent,
            group,
            session,
            cpu_ticks: 0,
            start_ticks: u64::from(pid),
            resident_pages: 0,
        }
    }

    #[test]
    fn a_stat_line_is_read_whatever_the_program_is_called() {
        let line = "4321 (Web (Content) :) x) S 100 4300 4200 0 -1 4194560 1 2 3 4 70 30 0 0 20 0 5 0 987654 1000000 250 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0";
        let stat = parse_stat(line).unwrap();
        assert_eq!(
            stat,
            Stat {
                pid: 4321,
                name: "Web (Content) :) x".into(),
                state: 'S',
                parent: 100,
                group: 4300,
                session: 4200,
                cpu_ticks: 100,
                start_ticks: 987_654,
                resident_pages: 250,
            }
        );
        assert!(stat.alive());
        assert!(!parse_stat(&line.replace(") S ", ") Z ")).unwrap().alive());
        assert_eq!(parse_stat("4321 (cut short) S 1"), None);
        assert_eq!(parse_stat(""), None);
    }

    #[test]
    fn this_process_is_listed_with_its_start_time_and_program() {
        let me = std::process::id();
        let list = processes().unwrap();
        let own = list.iter().find(|p| p.pid == me).unwrap();
        assert_ne!(own.started, 0);
        assert_eq!(own.started, creation_time(me));
        assert!(own.started <= now());
        let image = image_path(me).unwrap();
        assert_eq!(
            std::path::Path::new(&image),
            std::env::current_exe().unwrap()
        );
        // The test binary's name is longer than the kernel keeps.
        assert_eq!(own.name, file_name(&image));
        let usage = usage(me).unwrap();
        assert_eq!(usage.started, own.started);
        assert!(usage.working_set > 0);
        assert_eq!(creation_time(u32::MAX), 0);
        assert!(image_path(u32::MAX).is_none());
        assert!(usage_of_nothing());
    }

    fn usage_of_nothing() -> bool {
        usage(u32::MAX).is_none()
    }

    #[test]
    fn the_clock_counts_in_ticks_under_the_boots_tag() {
        let first = now();
        std::thread::sleep(Duration::from_millis(30));
        let second = now();
        assert!(second > first);
        assert_eq!(first >> TAG_SHIFT, second >> TAG_SHIFT);
        assert_eq!(started(0), boot_tag());
        assert_eq!(tag_of(""), 0);
        assert_eq!(tag_of("a") & ((1 << TAG_SHIFT) - 1), 0);
        assert_ne!(
            tag_of("3f2c8a52-0d3b-4a51-9d0e-6a7c2f1b9e10"),
            tag_of("7b1e44d9-5a8e-4c0f-8f33-0c9d6f2a1b77")
        );
        assert_eq!(file_name(r"C:\Games\Game.exe"), "Game.exe");
        assert_eq!(file_name("/usr/bin/steam"), "steam");
    }

    #[test]
    fn a_tree_is_its_session_its_group_and_their_descendants() {
        let stats = [
            // The command, leading its session and group.
            stat(10, 1, 10, 10),
            // Its child, and one that made its own group.
            stat(11, 10, 10, 10),
            stat(12, 11, 12, 10),
            // One that left the session but is still a child of the tree.
            stat(13, 12, 13, 13),
            stat(14, 13, 13, 13),
            // A daemon that left the session and its parent, and the rest of the PC.
            stat(15, 1, 15, 15),
            stat(1, 0, 1, 1),
            stat(20, 1, 20, 20),
        ];
        let pids = |root| -> Vec<u32> {
            tree_of(root, &stats)
                .into_iter()
                .map(|(pid, _)| pid)
                .collect()
        };
        assert_eq!(pids(10), [10, 11, 12, 13, 14]);
        assert_eq!(pids(20), [20]);
        assert_eq!(pids(99), [0u32; 0]);
    }
}
