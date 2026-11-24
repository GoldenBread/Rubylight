//! Tests for job-owned process spawning, piped I/O, window closing, and breakaway behavior.
use super::inspect::created_at;
use super::spawn::{PipeEnd, pipe_pair};
use super::tokens::{target_token, token};
use super::*;
use std::{collections::BTreeMap, ffi::OsString, time::Duration};
use windows::Win32::System::JobObjects::*;

mod security_session;
mod tokens_env;

#[test]
fn a_process_says_whether_windows_gave_it_administrator_rights() {
    let ping = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap())
        .join("System32")
        .join("ping.exe");
    // Runs where the test runs: as this user, elevated or not.
    let elevated = is_system() || token_elevated(&token().unwrap()).unwrap();
    let args = ["-n", "30", "127.0.0.1"].map(OsString::from);
    let process = Process::spawn(
        &ping,
        &args,
        None,
        Target::User { elevated },
        &BTreeMap::new(),
        true,
    )
    .unwrap();
    let rights = process.elevated();
    process.terminate().unwrap();
    assert_eq!(rights.unwrap(), elevated);
}

#[test]
fn a_pipe_end_can_be_used_by_its_creator() {
    use std::io::{Read, Write};
    let (child_end, parent_end) = pipe_pair(PipeEnd::Write).unwrap();
    let mut writer = std::fs::File::from(child_end);
    let mut reader = std::fs::File::from(parent_end);
    writer.write_all(b"ping").unwrap();
    drop(writer);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert_eq!(text, "ping");
}

#[test]
fn a_piped_child_reads_stdin_and_answers_on_stdout_and_stderr_inside_the_job() {
    use std::io::{Read, Write};
    let cmd =
        std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("System32\\cmd.exe");
    if !is_system()
        && target_token(&Target::User { elevated: false })
            .unwrap()
            .is_some()
    {
        // An elevated administrator host starts through the shell token, which cannot hand
        // over pipes; it must say so rather than start the program without them.
        let err = Process::spawn_piped(&cmd, &[], Target::User { elevated: false });
        assert!(format!("{:#}", err.err().unwrap()).contains("pipes"));
        return;
    }
    let system32 = cmd.parent().unwrap().to_path_buf();
    let target = || Target::User { elevated: false };
    // What goes to the child's stdin comes back on its stdout.
    let args = [OsString::from("^")];
    let mut echo = Process::spawn_piped(&system32.join("findstr.exe"), &args, target()).unwrap();
    echo.stdin.take().unwrap().write_all(b"hello\r\n").unwrap();
    let mut out = String::new();
    echo.stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(out.trim(), "hello");
    assert_eq!(echo.process.wait(Duration::from_secs(10)).unwrap(), 0);
    // What the child writes to stderr arrives on stderr, and its exit code is its own.
    let args = [OsString::from("nothing-by-this-name")];
    let mut failing = Process::spawn_piped(&system32.join("where.exe"), &args, target()).unwrap();
    let (mut out, mut err) = (String::new(), String::new());
    failing
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    failing
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(
        out.is_empty() && !err.is_empty(),
        "stdout {out:?} stderr {err:?}"
    );
    assert_eq!(failing.process.wait(Duration::from_secs(10)).unwrap(), 1);
    // The child runs as the signed-in user, never as the SYSTEM parent.
    if is_system() {
        let mut who = Process::spawn_piped(&system32.join("whoami.exe"), &[], target()).unwrap();
        let mut name = String::new();
        who.stdout
            .take()
            .unwrap()
            .read_to_string(&mut name)
            .unwrap();
        assert!(
            !name.trim().is_empty() && !name.to_lowercase().contains("system"),
            "{name:?}"
        );
        println!("the child ran as {}", name.trim());
    }
    // A child that does not end is ended by terminate, and its pipes then close.
    let args = [
        OsString::from("-n"),
        OsString::from("30"),
        OsString::from("127.0.0.1"),
    ];
    let mut long = Process::spawn_piped(&system32.join("ping.exe"), &args, target()).unwrap();
    assert_eq!(long.process.exit_code().unwrap(), None);
    long.process.terminate().unwrap();
    let mut rest = String::new();
    long.stdout
        .take()
        .unwrap()
        .read_to_string(&mut rest)
        .unwrap();
    assert!(long.process.wait(Duration::from_secs(10)).is_ok());
}

#[test]
fn close_image_asks_only_that_image_s_windows_and_follows_a_restarted_one() {
    use windows::Win32::UI::WindowsAndMessaging::*;
    // Whether `pid` owns a top-level window, shown or not.
    fn has_window(pid: u32) -> bool {
        unsafe extern "system" fn find(window: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the `&mut (u32, bool)` below, alive for the enumeration.
            unsafe {
                let found = &mut *(data.0 as *mut (u32, bool));
                let mut owner = 0;
                GetWindowThreadProcessId(window, Some(&mut owner));
                found.1 |= owner == found.0;
                BOOL(1)
            }
        }
        let mut found = (pid, false);
        // SAFETY: `found` outlives the synchronous enumeration that reads it through `find`.
        unsafe {
            let _ = EnumWindows(
                Some(find),
                LPARAM((&mut found as *mut (u32, bool)) as isize),
            );
        }
        found.1
    }
    let image_pid = |process: &Process, image: &str| {
        let job = process.process_ids().unwrap();
        processes()
            .unwrap()
            .into_iter()
            .find(|p| job.contains(&p.pid) && p.name.eq_ignore_ascii_case(image))
            .map(|p| p.pid)
    };
    let cmd =
        std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("System32\\cmd.exe");
    // cmd, which has no window, starts PowerShell holding a hidden form
    // that closes on WM_CLOSE, then keeps running itself. PowerShell
    // stands in for a Nucleus Co-op that restarted itself (not the job's
    // first process), cmd for a browser or game the job also holds.
    let form = "Add-Type -AssemblyName System.Windows.Forms; \
                    $f = New-Object System.Windows.Forms.Form; \
                    $f.add_FormClosed({ [System.Windows.Forms.Application]::ExitThread() }); \
                    $null = $f.Handle; [System.Windows.Forms.Application]::Run()";
    let line = format!(
        "{} /d /c start \"\" /b powershell.exe -NoProfile -NonInteractive -Command \"{form}\" & ping -n 60 127.0.0.1 >nul",
        quote(cmd.as_os_str()).unwrap()
    );
    // Runs where the elevated test runs: as this user, elevated or not.
    let elevated = is_system() || token_elevated(&token().unwrap()).unwrap();
    let process = Process::spawn_command(
        &cmd,
        &line,
        None,
        Target::User { elevated },
        &BTreeMap::new(),
        true,
        false,
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let shell = loop {
        if let Some(pid) = image_pid(&process, "powershell.exe").filter(|pid| has_window(*pid)) {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "PowerShell's form never showed up"
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(process.running_image("powershell.exe").unwrap());
    // Closing cmd sends PowerShell nothing: cmd has no window and stays,
    // and PowerShell keeps its form.
    assert!(
        !process
            .close_image("cmd.exe", Duration::from_millis(500))
            .unwrap()
    );
    assert!(has_window(shell), "PowerShell got a WM_CLOSE meant for cmd");
    assert_eq!(image_pid(&process, "powershell.exe"), Some(shell));
    // Closing PowerShell finds it though the job started cmd, and leaves
    // cmd running.
    assert!(
        process
            .close_image("powershell.exe", Duration::from_secs(20))
            .unwrap()
    );
    assert!(!process.running_image("powershell.exe").unwrap());
    assert!(process.running_image("cmd.exe").unwrap());
    process.terminate().unwrap();
}

#[test]
fn command_cancellation_ends_the_owned_process_before_returning() -> anyhow::Result<()> {
    let cancel = CommandCancellation::new()?;
    let elevated = user_environment().is_err();
    let child = Process::shell(
        "ping -n 120 127.0.0.1 >nul",
        None,
        elevated,
        &BTreeMap::new(),
    )?;
    cancel.cancel()?;
    let started = std::time::Instant::now();
    assert_eq!(
        child.wait_cancellable(Duration::from_secs(120), &cancel)?,
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(child.exit_code()?.is_some());
    assert_eq!(child.active_processes()?, 0);
    Ok(())
}

#[test]
fn the_process_list_names_this_test_and_its_program() {
    let me = std::process::id();
    let list = super::processes().unwrap();
    let this = list.iter().find(|p| p.pid == me).unwrap();
    assert!(this.started > 0);
    assert!(this.name.to_ascii_lowercase().ends_with(".exe"));
    let image = super::image_path(me).unwrap();
    assert!(image.ends_with(&this.name), "{image}");
    // Stopping a process list whose start times do not match leaves it alone.
    super::stop_processes(
        &std::collections::BTreeMap::from([(me, this.started + 1)]),
        std::time::Duration::ZERO,
    );
}

#[test]
fn dropping_the_job_terminates_the_shell_and_its_child() {
    let process = Process::shell(
        "ping -n 30 127.0.0.1 >nul",
        None,
        target_token(&Target::User { elevated: true }).is_ok(),
        &BTreeMap::new(),
    )
    .unwrap();
    let mut ids = [0usize; 34];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let count = loop {
        // SAFETY: `ids` outlives the call and its byte size is passed.
        unsafe {
            QueryInformationJobObject(
                Some(raw(&process.job)),
                JobObjectBasicProcessIdList,
                ids.as_mut_ptr().cast(),
                std::mem::size_of_val(&ids) as u32,
                None,
            )
            .unwrap();
        }
        // SAFETY: `ids` holds a JOBOBJECT_BASIC_PROCESS_ID_LIST whose second u32, inside the
        // array, is NumberOfProcessIdsInList.
        let count = unsafe { *ids.as_ptr().cast::<u32>().add(1) } as usize;
        if count >= 2 {
            break count;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child never joined the owned Windows job"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(count <= 32);
    let children: Vec<_> = ids[1..=count]
        .iter()
        .map(|id| {
            // SAFETY: OpenProcess has no memory preconditions and the returned handle is owned
            // once.
            owned(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, *id as u32).unwrap() })
        })
        .collect();
    drop(process);
    for child in children {
        assert_eq!(
            // SAFETY: `child` is an owned process handle, open for the wait.
            unsafe { WaitForSingleObject(raw(&child), 5000) },
            WAIT_OBJECT_0
        );
    }
}

/// A program the shell command started and left running, ended when the test is done
/// whatever its outcome.
struct LeftRunning(OwnedHandle);
impl LeftRunning {
    /// The shell `process` has exited by itself; the `ping` it started is still in its job.
    fn of(process: &Process) -> Self {
        assert_eq!(process.wait(Duration::from_secs(20)).unwrap(), 0);
        let ids: Vec<u32> = process.image_ids("ping.exe").unwrap().into_iter().collect();
        assert_eq!(ids.len(), 1, "{ids:?}");
        // SAFETY: OpenProcess has no memory preconditions and the returned handle is owned once.
        Self(owned(unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                ids[0],
            )
            .unwrap()
        }))
    }
    /// Whether it ended within `timeout`.
    fn ends_within(&self, timeout: Duration) -> bool {
        // SAFETY: `self.0` is an owned process handle, open for the wait.
        unsafe { WaitForSingleObject(raw(&self.0), timeout.as_millis() as u32) == WAIT_OBJECT_0 }
    }
}
impl Drop for LeftRunning {
    fn drop(&mut self) {
        // SAFETY: `self.0` is an owned process handle opened with PROCESS_TERMINATE.
        unsafe {
            let _ = TerminateProcess(raw(&self.0), 1);
            WaitForSingleObject(raw(&self.0), 5000);
        }
    }
}

fn starts_a_program_and_exits() -> Process {
    Process::shell(
        "start \"\" /b ping -n 60 127.0.0.1 >nul",
        None,
        target_token(&Target::User { elevated: true }).is_ok(),
        &BTreeMap::new(),
    )
    .unwrap()
}

#[test]
fn a_command_that_ended_by_itself_leaves_what_it_started_running() {
    // Issue 135: a preparation command's helper must still run for the app's next command.
    let process = starts_a_program_and_exits();
    let helper = LeftRunning::of(&process);
    process.leave_children_running().unwrap();
    drop(process);
    assert!(!helper.ends_within(Duration::from_secs(2)));
}

/// Run by [`what_a_command_leaves_running_outlives_the_hosts_own_job`] in a copy of this test
/// program that sits in a kill-on-close job, as the service's host does: starts a command that
/// leaves a program running, and prints that program's pid and creation time.
#[test]
#[ignore = "run inside a parent job by what_a_command_leaves_running_outlives_the_hosts_own_job"]
fn leave_a_program_running_inside_a_parent_job() {
    if std::env::var_os("RUBYLIGHT_TEST_IN_PARENT_JOB").is_none() {
        return;
    }
    // The parent puts this process in its job before it says go.
    let mut go = String::new();
    std::io::stdin().read_line(&mut go).unwrap();
    let process = Process::shell_leaving_children(
        "start \"\" /b ping -n 60 127.0.0.1 >nul",
        None,
        target_token(&Target::User { elevated: true }).is_ok(),
        &BTreeMap::new(),
    )
    .unwrap();
    let helper = LeftRunning::of(&process);
    process.leave_children_running().unwrap();
    drop(process);
    // SAFETY: the handle is open.
    let pid = unsafe { GetProcessId(raw(&helper.0)) };
    println!("HELPER {pid} {}", created_at(raw(&helper.0)));
    // The parent ends it from here on.
    std::mem::forget(helper);
}

#[test]
fn what_a_command_leaves_running_outlives_the_hosts_own_job() {
    use std::io::{BufRead, Write};
    use std::os::windows::io::AsRawHandle;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // SAFETY: a null job asks about this process's own job; `limits` is a writable local of the
    // size passed.
    let in_job = unsafe {
        QueryInformationJobObject(
            None,
            JobObjectExtendedLimitInformation,
            (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            None,
        )
    }
    .is_ok();
    if in_job && (limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_BREAKAWAY_OK).0 == 0 {
        // The job this test runs in forbids leaving it, so nothing started here can.
        eprintln!("skipped: this test runs in a job that forbids breakaway");
        return;
    }
    // The service's job for the host: its processes end when it closes, and may leave it.
    // SAFETY: the limits are a live local of the size passed, and the new handle is owned once.
    let parent = unsafe {
        let job = owned(CreateJobObjectW(None, PCWSTR::null()).unwrap());
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        SetInformationJobObject(
            raw(&job),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .unwrap();
        job
    };
    let mut host = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
            "process::tests::leave_a_program_running_inside_a_parent_job",
        ])
        .env("RUBYLIGHT_TEST_IN_PARENT_JOB", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // SAFETY: both handles are open for the call.
    let assigned = unsafe { AssignProcessToJobObject(raw(&parent), HANDLE(host.as_raw_handle())) };
    if let Err(error) = assigned {
        let _ = host.kill();
        let _ = host.wait();
        panic!("cannot put the test host in its job: {error}");
    }
    writeln!(host.stdin.take().unwrap(), "go").unwrap();
    let mut found = None;
    for line in std::io::BufReader::new(host.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        // libtest may print the test's name before it on the same line.
        if let Some((_, rest)) = line.split_once("HELPER ") {
            let (pid, created) = rest.split_once(' ').unwrap();
            found = Some((pid.parse::<u32>().unwrap(), created.parse::<u64>().unwrap()));
        }
    }
    assert!(host.wait().unwrap().success());
    let (pid, created) = found.expect("the test host named no program");
    // SAFETY: OpenProcess has no memory preconditions and the returned handle is owned once. The
    // parent job is still open, so the program cannot have ended and given away its pid.
    let helper = LeftRunning(owned(unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
        .unwrap()
    }));
    assert_eq!(created_at(raw(&helper.0)), created);
    // The host exits: its job closes.
    drop(parent);
    assert!(!helper.ends_within(Duration::from_secs(2)));
}

#[test]
fn a_command_dropped_without_leaving_its_children_takes_them_with_it() {
    // A command that was stopped, timed out or whose wait failed is dropped as it is.
    let process = starts_a_program_and_exits();
    let helper = LeftRunning::of(&process);
    drop(process);
    assert!(helper.ends_within(Duration::from_secs(10)));
}
