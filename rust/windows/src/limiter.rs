//! Stream-owned limiter transactions. Durable originals precede every mutation.
use crate::rtss;
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    config::Config,
    framegen::{Policy, Provider, Rate},
    state::write_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
#[derive(Clone, Serialize, Deserialize)]
struct RtssChange {
    root: PathBuf,
    before: BTreeMap<String, Option<u32>>,
    applied: BTreeMap<String, Option<u32>>,
    disabled: bool,
    /// What this host gave RTSS in place of `applied` while the stream's display was too
    /// slow for the cap ([`follow_display`]). A profile that holds these is the host's
    /// doing as well, so restoring replaces them like `applied`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stood_down: Option<BTreeMap<String, Option<u32>>>,
}
impl RtssChange {
    /// Whether the profile's `key` still holds a value this host wrote. A user's edit made
    /// after it takes precedence.
    fn ours(&self, key: &str, current: &BTreeMap<String, Option<u32>>) -> bool {
        current.get(key) == self.applied.get(key)
            || self
                .stood_down
                .as_ref()
                .and_then(|values| values.get(key))
                .is_some_and(|value| current.get(key) == Some(value))
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Journal {
    version: u32,
    rtss: Option<RtssChange>,
    /// NVIDIA driver profile changes a 2.6 host journalled. 3.0 cannot restore
    /// them, but must not lose track of them silently; see `take_stale_nvidia`.
    #[serde(default)]
    nvidia: Vec<Json>,
}
#[derive(Default)]
struct State {
    users: usize,
    active: String,
    rate: Option<Rate>,
    directory: PathBuf,
    process: Option<(PathBuf, crate::process::Process)>,
    message: String,
    /// The automatic cap [`follow_display`] takes off and puts back.
    follow: Option<Follow>,
}
/// A virtual display's automatic cap, applied through RTSS.
struct Follow {
    /// Which stream's cap this is: a change on its way belongs to one.
    id: u64,
    root: PathBuf,
    rate: Rate,
    /// The cap is off: the stream's display refreshes too slowly for it.
    standing_down: bool,
    /// A change is on its way on its own thread.
    changing: bool,
    /// A change failed; the cap stays as it is until the stream ends.
    failed: bool,
    /// A second stream joined: the cap is no longer one display's to decide, and stays on
    /// until the last stream ends.
    shared: bool,
}
fn next_follow() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}
fn path(directory: &Path) -> PathBuf {
    directory.join("frame-limiter-recovery.json")
}
fn load(directory: &Path) -> Result<Journal> {
    match std::fs::metadata(path(directory)) {
        Ok(m) if m.len() <= 1024 * 1024 => {
            let j: Journal = serde_json::from_slice(&std::fs::read(path(directory))?)?;
            if j.version != 1 {
                bail!("unsupported frame limiter recovery version");
            }
            Ok(j)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Journal {
            version: 1,
            ..Default::default()
        }),
        _ => bail!("frame limiter recovery is unreadable or exceeds its size limit"),
    }
}
fn persist(directory: &Path, journal: &Journal) -> Result<()> {
    write_json(&path(directory), journal)?;
    crate::display_recovery::external(true)
}
/// The NVIDIA driver settings a 2.6 host changed for a stream and, after a
/// crash, left in `frame-limiter-recovery.json` (a hybrid AMD and NVIDIA
/// laptop). 3.0 has no NVIDIA code to put them back, so name them for the user
/// and drop the entries, which would otherwise be retained and ignored forever.
/// The warning shows once; if the journal cannot be written it shows again at
/// the next start.
pub fn take_stale_nvidia(directory: &Path) -> Option<String> {
    let mut journal = match load(directory) {
        Ok(journal) => journal,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "frame limiter recovery journal is unreadable");
            return None;
        }
    };
    let message = stale_nvidia_warning(&journal.nvidia)?;
    journal.nvidia.clear();
    if let Err(error) = write_json(&path(directory), &journal) {
        tracing::warn!(error = %format!("{error:#}"), "the stale NVIDIA entries could not be removed from the recovery journal");
    }
    Some(message)
}
fn stale_nvidia_warning(entries: &[Json]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for id in entries.iter().filter_map(|entry| entry["id"].as_u64()) {
        let name = match id {
            0x1083_5002 => "Max Frame Rate".to_string(),
            0x00a8_79cf => "Vertical sync".to_string(),
            0xb0d3_84c0 | 0xb0cc_0875 | 0x0005_f543 | 0x1083_5000 | 0x20d6_90f8 => {
                "Smooth Motion".to_string()
            }
            other => format!("driver setting {other:#010x}"),
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let names = if names.is_empty() {
        "frame limit and vertical sync".to_string()
    } else {
        names.join(", ")
    };
    Some(format!(
        "A previous Rubylight 2.6 host stopped before it restored NVIDIA driver settings it changed for a stream ({names}). Rubylight 3.0 cannot restore them. Open NVIDIA Control Panel, Manage 3D settings, and reset the Global profile and the butterpollo.exe program profile."
    ))
}
/// Also used by the Rust crash watcher. Failures leave the journal intact.
pub fn recover(directory: &Path) -> Result<()> {
    restore(directory, &mut None)
}
/// `owned`: the RTSS this host started for the stream. Closing it ends every
/// live change, and RTSS reads the restored profile when it next starts, so
/// nothing is left to restore through its SDK (Vibepollo also closes it).
fn restore(directory: &Path, owned: &mut Option<(PathBuf, crate::process::Process)>) -> Result<()> {
    let mut journal = load(directory)?;
    if let Some(change) = journal.rtss.as_ref() {
        let text = rtss::read(&change.root)?;
        let current = rtss::properties(&text)?;
        let mut restore = BTreeMap::new();
        // A user edit made after our application takes precedence on restore.
        for key in rtss::KEYS {
            if change.ours(key, &current) {
                restore.insert(key.into(), change.before.get(key).copied().flatten());
            }
        }
        // Restore the durable profile even if RTSS's message loop is stalled.
        // Keep the journal until the helper also confirms the live state.
        if !restore.is_empty() {
            rtss::write_profile(&change.root, &restore)?;
        }
        if let Some((_, process)) = owned.take_if(|(root, _)| *root == change.root) {
            process
                .stop()
                .context("close the RTSS started for the stream")?;
        } else if rtss::available(&change.root) {
            let _process = rtss::start(&change.root)?;
            let before = rtss::wait_ready(&change.root)
                .context("ask RTSS for its state before restoring")?;
            let disabled = if before.flags & 4 == 0 {
                Some(change.disabled)
            } else {
                None
            };
            let restored = rtss::apply(&change.root, &restore, disabled)
                .context("give RTSS its original frame limit")?;
            let unsupported = confirmed(&restore, &restored.values, false)
                .map_err(|key| anyhow::anyhow!("RTSS did not confirm restoration of {key}"))?;
            warn_unsupported(&unsupported);
            if disabled.is_some() && (restored.flags & 4 != 0) != change.disabled {
                bail!("RTSS did not restore its original enable state");
            }
        } else if rtss::running(&change.root) {
            bail!(
                "RTSS is still running but its hooks are unavailable; retaining exclusive limiter ownership"
            );
        }
        journal.rtss = None;
        write_json(&path(directory), &journal)?;
    }
    Ok(())
}
/// Whether RTSS's reply (`reported`) holds each value of `wanted`: the
/// key it holds another value for, or the keys it doesn't report at all,
/// which this RTSS doesn't support (its profile file still has them, from
/// [`rtss::write_profile`]). A key originally left out (None) is RTSS's
/// default and can't be told apart. A limit RTSS states another way is the
/// same cap: 2997/25 as 11988/100, a denominator of 0 as 1, and any
/// denominator without a limit (Limit 0 is no cap). Otherwise such a
/// restoration stayed pending forever, holding the limiter at every stream.
/// An unreported `Limit` is never confirmed, and when `applying` a stream's
/// cap, neither is an unreported denominator other than 1.
fn confirmed(
    wanted: &BTreeMap<String, Option<u32>>,
    reported: &BTreeMap<String, Option<u32>>,
    applying: bool,
) -> std::result::Result<Vec<String>, String> {
    let value = |map: &BTreeMap<String, Option<u32>>, key: &str| map.get(key).copied().flatten();
    let cap = |limit: u32, denominator: u32| (u64::from(limit), u64::from(denominator.max(1)));
    let same_cap = || {
        let (Some(limit), Some(denominator)) = (
            value(reported, "Limit"),
            value(reported, "LimitDenominator"),
        ) else {
            return false;
        };
        let wanted_limit = match wanted.get("Limit") {
            Some(wanted) => wanted.unwrap_or(0),
            None => limit,
        };
        let wanted_denominator = match wanted.get("LimitDenominator") {
            Some(wanted) => wanted.unwrap_or(1),
            None => denominator,
        };
        let (have, have_by) = cap(limit, denominator);
        let (want, want_by) = cap(wanted_limit, wanted_denominator);
        (have == 0 && want == 0) || have * want_by == want * have_by
    };
    let mut unsupported = Vec::new();
    for (key, want) in wanted {
        let Some(want) = *want else { continue };
        match value(reported, key) {
            // No cap is confirmed without the limit itself, and a stream's
            // fractional cap not without its denominator: RTSS could be
            // holding 2997 fps for 29.97.
            None if key == "Limit" => return Err(key.clone()),
            None if applying && key == "LimitDenominator" && want.max(1) != 1 => {
                return Err(key.clone());
            }
            None => unsupported.push(key.clone()),
            Some(got) if got == want => {}
            Some(_) if matches!(key.as_str(), "Limit" | "LimitDenominator") && same_cap() => {}
            Some(_) => return Err(key.clone()),
        }
    }
    Ok(unsupported)
}
/// Says once per host start which keys this RTSS doesn't report
/// ([`confirmed`]).
fn warn_unsupported(keys: &[String]) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !keys.is_empty() && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(
            keys = keys.join(", "),
            "this RTSS does not report these frame limiter settings, so Rubylight sets them in its Global profile but cannot confirm them live; RTSS 7.3.7 or newer reports them"
        );
    }
}
/// What the console says when a stream's cap went to RTSS but RTSS didn't
/// report `keys` back ([`confirmed`]).
fn unconfirmed_message(keys: &[String]) -> String {
    format!(
        "RTSS does not report {}, so Rubylight could not confirm them; RTSS 7.3.7 or newer reports them",
        keys.join(", ")
    )
}
/// Whether a stream that ends up with no limiter provider is a fault worth a
/// warning in the host log: the user asked for a cap (`frame_limiter_enable`,
/// or RTSS by name) or a provider failed. `frame_limiter_provider = none`
/// turns limiting off on purpose, and the automatic virtual-display cap on a
/// host without RTSS simply stays off; both are quiet, as the console's
/// warning ([`limiter_warning`]) already is.
fn missing_provider_is_a_fault(explicit: bool, provider: Provider, error: &str) -> bool {
    provider != Provider::None && (explicit || provider == Provider::Rtss || !error.is_empty())
}
/// `explicit`: the user asked for a cap ("Limit every stream" or a named
/// provider), rather than the automatic virtual-display limit, which simply
/// stays off on a host without RTSS.
fn limiter_warning(
    enabled: bool,
    explicit: bool,
    active: &str,
    error: &str,
    rates: (Option<Rate>, Option<Rate>),
) -> Option<String> {
    if !enabled && rates.1.is_none() {
        return None;
    }
    if active == "none" || active.is_empty() {
        if error.is_empty() && !explicit {
            return None;
        }
        let cause = if error.is_empty() {
            String::new()
        } else {
            format!(" ({error})")
        };
        Some(format!(
            "Frame limiter not applied: no usable RTSS{cause}. Game frame pacing may be uneven; install RTSS or set an in-game frame limit."
        ))
    } else if !error.is_empty() {
        Some(format!(
            "Frame limiter settings were not fully applied ({error}). Check RTSS before relying on the requested cap."
        ))
    } else if let Some(rate) = rates.1
        && rates.0 != rates.1
    {
        Some(if rates.0.is_some() {
            format!(
                "Another stream owns the frame limiter at {rate} fps; this session did not apply its requested cap. Match the other stream or disconnect it before changing the global limit."
            )
        } else {
            format!(
                "Another stream's {rate} fps frame limit also applies to games in this session. Disconnect that stream to remove the cap."
            )
        })
    } else {
        None
    }
}

pub struct Lease;
impl Lease {
    pub fn warning(&self, policy: &Policy, explicit: bool) -> Option<String> {
        let state = state().lock().unwrap();
        limiter_warning(
            policy.enabled,
            explicit || policy.provider == Provider::Rtss,
            &state.active,
            &state.message,
            (policy.enabled.then_some(policy.rate), state.rate),
        )
    }
    pub fn acquire(directory: &Path, config: &Config, policy: &Policy) -> Result<Self> {
        let mut state = state().lock().unwrap();
        if state.users != 0 {
            state.users += 1;
            // Another stream joins: the cap is no longer one display's to stand down for.
            if state.follow.as_ref().is_some_and(|f| f.standing_down) {
                change_followed(&mut state, false, None);
            }
            if let Some(follow) = state.follow.as_mut() {
                follow.shared = true;
            }
            return Ok(Self);
        }
        state.directory = directory.into();
        state.active = "none".into();
        state.rate = policy.enabled.then_some(policy.rate);
        state.message.clear();
        state.follow = None;
        if let Err(error) = recover(directory) {
            state.active = "rtss".into();
            state.message = format!("{error:#}");
            tracing::warn!(error = %state.message, "frame limiter recovery remains pending; retaining exclusive limiter ownership");
            state.users = 1;
            return Ok(Self);
        }
        let mut journal = load(directory)?;
        let root = rtss::root(config);
        let use_rtss = policy.enabled
            && matches!(policy.provider, Provider::Auto | Provider::Rtss)
            && rtss::available(&root);
        if !use_rtss
            || state
                .process
                .as_ref()
                .is_some_and(|(owned_root, _)| owned_root != &root)
        {
            state.process.take();
        }
        if use_rtss {
            let applied = apply_cap(
                directory,
                &mut journal,
                &root,
                (policy.rate, policy.sync_limiter),
                &mut state.process,
            );
            match applied {
                Ok(unsupported) => {
                    state.active = "rtss".into();
                    state.follow = policy.follows_display.then(|| Follow {
                        id: next_follow(),
                        root: root.clone(),
                        rate: policy.rate,
                        standing_down: false,
                        changing: false,
                        failed: false,
                        shared: false,
                    });
                    let version = rtss::version(&root);
                    if unsupported.is_empty() {
                        tracing::info!(rate = ?policy.rate.rational(), sync_limiter = policy.sync_limiter, rtss_version = ?version, profile_sdk = rtss::profile_sdk(version), "RTSS frame limit applied and verified");
                    } else {
                        state.message = unconfirmed_message(&unsupported);
                        tracing::warn!(rate = ?policy.rate.rational(), sync_limiter = policy.sync_limiter, rtss_version = ?version, profile_sdk = rtss::profile_sdk(version), "RTSS frame limit applied but not verified: {}", state.message);
                    }
                }
                Err(error) => {
                    state.message = format!("{error:#}");
                    tracing::warn!(root = %root.display(), error = %state.message, "RTSS frame limiter could not be applied");
                    if journal.rtss.is_some()
                        && let Err(error) = restore(directory, &mut state.process)
                    {
                        tracing::warn!(error = %format!("{error:#}"), "RTSS frame limiter could not be restored after the failure");
                        state.active = "rtss".into();
                    }
                }
            }
        }
        if state.active == "none" && policy.enabled {
            if missing_provider_is_a_fault(
                config.boolean("frame_limiter_enable", false),
                policy.provider,
                &state.message,
            ) {
                tracing::warn!(provider = policy.provider.name(), rtss_root = %root.display(), rtss_available = rtss::available(&root), error = %state.message, "frame limiter has no available provider");
            } else {
                tracing::debug!(provider = policy.provider.name(), rtss_root = %root.display(), rtss_available = rtss::available(&root), "frame limiter stays off: no provider was asked for or is available");
            }
        }
        state.users = 1;
        Ok(Self)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut guard = state().lock().unwrap();
        let state = &mut *guard;
        state.users -= 1;
        if state.users != 0 {
            return;
        }
        state.follow = None;
        match restore(&state.directory, &mut state.process) {
            Ok(()) => {
                let _ = crate::display_recovery::external(false);
                state.active = "none".into();
                state.message.clear();
                state.process.take();
            }
            Err(error) => {
                state.message = format!("{error:#}");
                tracing::warn!(error = %state.message, "frame limiter restoration remains pending");
            }
        }
    }
}
/// Journal RTSS's values as they are and give it a stream's cap. `process`: the RTSS this
/// host started, if it had to.
fn apply_cap(
    directory: &Path,
    journal: &mut Journal,
    root: &Path,
    (rate, sync_limiter): (Rate, u32),
    process: &mut Option<(PathBuf, crate::process::Process)>,
) -> Result<Vec<String>> {
    // A failed restoration retains our process until recovery
    // succeeds. start() returns None for that still-running RTSS;
    // replacing its owner with None would kill it on reconnect.
    if let Some(started) = rtss::start(root)? {
        *process = Some((root.into(), started));
    }
    let reply = rtss::wait_ready(root)?;
    let text = rtss::read(root)?;
    let (numerator, denominator) = rate.rational();
    let change = RtssChange {
        root: root.into(),
        before: rtss::properties(&text)?,
        applied: BTreeMap::from([
            ("Limit".into(), Some(numerator)),
            ("LimitDenominator".into(), Some(denominator)),
            ("SyncLimiter".into(), Some(sync_limiter)),
        ]),
        disabled: reply.flags & 4 != 0,
        stood_down: None,
    };
    journal.rtss = Some(change.clone());
    persist(directory, journal)?;
    let reply = rtss::apply(root, &change.applied, Some(false))?;
    if reply.flags & 4 != 0 {
        bail!("RTSS did not confirm the requested rational frame limit");
    }
    confirmed(&change.applied, &reply.values, true)
        .map_err(|_| anyhow::anyhow!("RTSS did not confirm the requested rational frame limit"))
}
/// The stream's display refreshes at `display` now: take a virtual display's automatic cap
/// off while that is too slow for it ([`butterpollo_core::framegen::cap_suits_display`]),
/// and put it back when the display is fast again. A cap the user asked for, a second
/// stream's and a host without RTSS are left alone. Called from the stream's loop with each
/// timing window, so it never waits: the change is made on a thread of its own.
pub fn follow_display(display: Option<Rate>) {
    let Some(display) = display else {
        return;
    };
    // A stream starting or ending holds the lock across its RTSS calls; the next window asks
    // again.
    let Ok(mut guard) = state().try_lock() else {
        return;
    };
    let Some(stand_down) = wanted_change(&guard, display) else {
        return;
    };
    let Some(follow) = guard.follow.as_mut() else {
        return;
    };
    let id = follow.id;
    // Started under the lock, which the thread takes first: it runs once this returns, and a
    // thread that could not start leaves nothing to undo.
    let spawned = std::thread::Builder::new()
        .name("limiter-display".into())
        .spawn(move || {
            let mut guard = self::state().lock().unwrap();
            let state = &mut *guard;
            // The stream may have ended, and another begun, since this was asked for.
            let Some(follow) = state.follow.as_mut().filter(|follow| follow.id == id) else {
                return;
            };
            follow.changing = false;
            if wanted_change(state, display) == Some(stand_down) {
                change_followed(state, stand_down, Some(display));
            }
        });
    match spawned {
        Ok(_) => follow.changing = true,
        Err(error) => {
            tracing::warn!(%error, "the frame limiter could not follow the display: no thread")
        }
    }
}
/// Whether the followed cap has to come off (`true`) or go back on for a display at
/// `display`, or `None` when it is as it should be, a change is under way, or there is no
/// cap one stream's display decides.
fn wanted_change(state: &State, display: Rate) -> Option<bool> {
    let follow = state.follow.as_ref()?;
    if state.users != 1
        || state.active != "rtss"
        || follow.changing
        || follow.failed
        || follow.shared
    {
        return None;
    }
    let stand_down = !butterpollo_core::framegen::cap_suits_display(follow.rate, display);
    (stand_down != follow.standing_down).then_some(stand_down)
}
/// Take the followed cap off or put it back, in place: the journal keeps what RTSS held
/// before the stream, and the stream's end restores that either way.
fn change_followed(state: &mut State, stand_down: bool, display: Option<Rate>) {
    let Some(follow) = state.follow.as_mut() else {
        return;
    };
    let result = retarget(&state.directory, &follow.root, stand_down);
    let display_hz = display.map(|rate| f64::from(rate.0) / 1000.);
    match result {
        Ok(()) => {
            follow.standing_down = stand_down;
            if stand_down {
                tracing::info!(rate = ?follow.rate.rational(), display_hz, "the stream's display now refreshes too slowly for the automatic frame limit, so the limit is off and the display's refresh paces the game");
            } else {
                tracing::info!(rate = ?follow.rate.rational(), display_hz, "the automatic frame limit is back on");
            }
        }
        Err(error) => {
            follow.failed = true;
            // RTSS may hold the new value without having confirmed it: go by its profile.
            let off = rtss::read(&follow.root)
                .and_then(|profile| rtss::properties(&profile))
                .is_ok_and(|current| {
                    no_limit()
                        .iter()
                        .all(|(key, value)| current.get(key) == Some(value))
                });
            follow.standing_down = off;
            tracing::warn!(stand_down, limit_off = off, display_hz, error = %format!("{error:#}"), "the automatic frame limit could not follow the display's refresh rate; RTSS keeps what it has now until the stream ends");
        }
    }
}
/// No limit, as RTSS holds it.
fn no_limit() -> BTreeMap<String, Option<u32>> {
    BTreeMap::from([
        ("Limit".into(), Some(0)),
        ("LimitDenominator".into(), Some(1)),
    ])
}
/// Give RTSS no limit in place of the stream's cap (`stand_down`), or the cap again. RTSS
/// keeps running and the journal keeps the user's own values: a limit of theirs is the
/// stream's to replace until it ends, and would run beside the display's refresh as the
/// cap did (their 120 fps limit on a 60 Hz mode left 3.3 % of the frame intervals off, the
/// 2026-11-19 run). The journal names both values as this host's before RTSS gets the
/// first, so a host that dies in either state restores the user's. A limit someone changed
/// during the stream is theirs and is left alone.
fn retarget(directory: &Path, root: &Path, stand_down: bool) -> Result<()> {
    let mut journal = load(directory)?;
    let Some(change) = journal.rtss.as_mut().filter(|change| change.root == root) else {
        bail!("the stream's frame limit is no longer in the recovery journal");
    };
    let current = rtss::properties(&rtss::read(root)?)?;
    let wanted = if stand_down {
        no_limit()
    } else {
        change.applied.clone()
    };
    if let Some(key) = wanted.keys().find(|key| !change.ours(key, &current)) {
        bail!("RTSS's {key} was changed during the stream");
    }
    if stand_down && change.stood_down.is_none() {
        change.stood_down = Some(wanted.clone());
        persist(directory, &journal)?;
    }
    // Off leaves RTSS's own switch for its limiter alone; the cap needs it on.
    let reply = rtss::apply(root, &wanted, (!stand_down).then_some(false))?;
    if !stand_down && reply.flags & 4 != 0 {
        bail!("RTSS did not confirm the requested rational frame limit");
    }
    let unsupported = confirmed(&wanted, &reply.values, !stand_down)
        .map_err(|key| anyhow::anyhow!("RTSS did not confirm {key}"))?;
    warn_unsupported(&unsupported);
    // The cap is back, so no limit is no longer this host's value: one that somebody
    // turns off from here on is theirs. A journal that cannot be written keeps the claim.
    if !stand_down
        && journal
            .rtss
            .as_mut()
            .is_some_and(|change| change.stood_down.take().is_some())
    {
        let _ = persist(directory, &journal);
    }
    Ok(())
}
/// The rate RTSS holds games to while a stream holds the frame limiter, if it limits.
pub fn active_rate() -> Option<Rate> {
    let state = state().lock().unwrap();
    (state.users > 0
        && state.active == "rtss"
        && !state.follow.as_ref().is_some_and(|f| f.standing_down))
    .then_some(state.rate)
    .flatten()
}
pub fn status(config: &Config) -> Json {
    let root = rtss::root(config);
    let available = rtss::available(&root);
    let state = state().lock().unwrap();
    let configured = config.get("rtss_install_path", "");
    json!({"status":true,"enabled":config.boolean("frame_limiter_enable",false),
        "configured_provider":Provider::parse(config.get("frame_limiter_provider","auto")).name(),
        "active_provider":if state.active.is_empty(){"none"}else{&state.active},
        "rtss_available":available,"configured_path":configured,"path_configured":!configured.is_empty(),
        "resolved_path":root,"path_exists":root.is_dir(),"hooks_found":available,
        "rtss_version":rtss::version(&root).map(|v| v.map(|part| part.to_string()).join(".")),"profile_found":root.join("Profiles/Global").is_file(),
        "can_bootstrap_profile":available,"process_running":rtss::running(&root),
        "message":if state.message.is_empty(){"Limiter settings apply during streaming and restore after the last stream."}else{&state.message}})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_journal_with_nvidia_entries_is_not_dropped_silently() {
        let directory = std::env::temp_dir().join(format!("limiter-nvidia-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            path(&directory),
            r#"{"version":1,"rtss":null,"nvidia":[{"id":277041154,"before":{"effective":0,"override_value":null},"applied":60},{"id":11041231,"before":{"effective":0,"override_value":null},"applied":134497095}]}"#,
        )
        .unwrap();
        // The entries survive loading and are written back with the journal.
        let journal = load(&directory).unwrap();
        assert_eq!(journal.nvidia.len(), 2);
        let message = take_stale_nvidia(&directory).expect("a warning");
        assert!(message.contains("Max Frame Rate"), "{message}");
        assert!(message.contains("Vertical sync"), "{message}");
        assert!(message.contains("NVIDIA Control Panel"), "{message}");
        // Warned once: the entries are gone, the rest of the journal stays.
        assert!(load(&directory).unwrap().nvidia.is_empty());
        assert!(take_stale_nvidia(&directory).is_none());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// A game that sets the stream's display to 60 Hz under the automatic
    /// 60 fps cap: the cap comes off, and returns with a fast mode. One stream,
    /// RTSS and an untroubled cap only.
    #[test]
    fn the_automatic_cap_follows_one_streams_display() {
        let followed = || State {
            users: 1,
            active: "rtss".into(),
            rate: Some(Rate(60_000)),
            follow: Some(Follow {
                id: 1,
                root: PathBuf::new(),
                rate: Rate(60_000),
                standing_down: false,
                changing: false,
                failed: false,
                shared: false,
            }),
            ..State::default()
        };
        let mut state = followed();
        assert_eq!(wanted_change(&state, Rate(1_000_000)), None);
        assert_eq!(wanted_change(&state, Rate(120_000)), None);
        assert_eq!(wanted_change(&state, Rate(60_000)), Some(true));
        assert_eq!(wanted_change(&state, Rate(59_000)), Some(true));
        state.follow.as_mut().unwrap().standing_down = true;
        assert_eq!(wanted_change(&state, Rate(60_000)), None);
        assert_eq!(wanted_change(&state, Rate(1_000_000)), Some(false));
        // Nothing while a change is under way or after one failed.
        state.follow.as_mut().unwrap().changing = true;
        assert_eq!(wanted_change(&state, Rate(1_000_000)), None);
        let mut state = followed();
        state.follow.as_mut().unwrap().failed = true;
        assert_eq!(wanted_change(&state, Rate(60_000)), None);
        // A second stream shares the cap, also once the first has left; a cap the user
        // asked for has no `follow`.
        let mut state = followed();
        state.users = 2;
        assert_eq!(wanted_change(&state, Rate(60_000)), None);
        let mut state = followed();
        state.follow.as_mut().unwrap().shared = true;
        assert_eq!(wanted_change(&state, Rate(60_000)), None);
        let mut state = followed();
        state.follow = None;
        assert_eq!(wanted_change(&state, Rate(60_000)), None);
        let mut state = followed();
        state.active = "none".into();
        assert_eq!(wanted_change(&state, Rate(60_000)), None);
    }

    /// The stream's end, or the crash watcher, finds the profile with the cap or
    /// with the limit off (the stream's display was too slow for the cap): both
    /// are the host's, and the user's own values come back. A limit the user set
    /// during the stream stays.
    #[test]
    fn a_stood_down_limit_is_restored_like_the_cap() {
        let values = |limit: u32, denominator: u32, sync: u32| {
            BTreeMap::from([
                ("Limit".to_string(), Some(limit)),
                ("LimitDenominator".to_string(), Some(denominator)),
                ("SyncLimiter".to_string(), Some(sync)),
            ])
        };
        for (name, during, after) in [
            ("cap", values(60, 1, 2), values(120, 1, 1)),
            ("off", values(0, 1, 2), values(120, 1, 1)),
            // 90 is the user's; the sync mode still holds the host's value.
            ("edited", values(90, 1, 2), values(90, 1, 1)),
        ] {
            let directory = std::env::temp_dir()
                .join(format!("limiter-stood-down-{name}-{}", std::process::id()));
            let root = directory.join("rtss");
            std::fs::create_dir_all(root.join("Profiles")).unwrap();
            let profile = |values: &BTreeMap<String, Option<u32>>| {
                rtss::replace("[Framerate]\nCustom=keep\n", values).unwrap()
            };
            std::fs::write(root.join("Profiles/Global"), profile(&during)).unwrap();
            let journal = Journal {
                version: 1,
                rtss: Some(RtssChange {
                    root: root.clone(),
                    before: values(120, 1, 1),
                    applied: values(60, 1, 2),
                    disabled: false,
                    stood_down: Some(no_limit()),
                }),
                nvidia: Vec::new(),
            };
            write_json(&path(&directory), &journal).unwrap();
            recover(&directory).unwrap();
            let restored = rtss::properties(&rtss::read(&root).unwrap()).unwrap();
            assert_eq!(restored, after, "{name}");
            assert!(load(&directory).unwrap().rtss.is_none(), "{name}");
            std::fs::remove_dir_all(&directory).unwrap();
        }
        // A journal written before the limit could stand down reads as before.
        let old: RtssChange = serde_json::from_str(
            r#"{"root":"C:/rtss","before":{"Limit":120},"applied":{"Limit":60},"disabled":false}"#,
        )
        .unwrap();
        assert!(old.stood_down.is_none());
        assert!(!serde_json::to_string(&old).unwrap().contains("stood_down"));
    }

    /// A stream to a virtual display with `frame_limiter_provider = none` used
    /// to warn "frame limiter has no available provider" at every launch,
    /// although limiting was turned off on purpose.
    #[test]
    fn a_missing_provider_is_a_fault_only_when_a_cap_was_asked_for() {
        assert!(!missing_provider_is_a_fault(false, Provider::None, ""));
        assert!(!missing_provider_is_a_fault(true, Provider::None, ""));
        // The automatic virtual-display cap on a host without RTSS.
        assert!(!missing_provider_is_a_fault(false, Provider::Auto, ""));
        assert!(missing_provider_is_a_fault(true, Provider::Auto, ""));
        assert!(missing_provider_is_a_fault(false, Provider::Rtss, ""));
        assert!(missing_provider_is_a_fault(
            false,
            Provider::Auto,
            "RTSS did not confirm"
        ));
    }

    #[test]
    fn limiter_reports_missing_and_failed_providers() {
        assert!(
            limiter_warning(true, true, "none", "", (None, None))
                .unwrap()
                .contains("not applied")
        );
        assert!(
            limiter_warning(true, true, "rtss", "recovery pending", (None, None))
                .unwrap()
                .contains("recovery pending")
        );
        assert!(limiter_warning(false, true, "none", "", (None, None)).is_none());
        // The automatic virtual-display cap on a host without RTSS is not a fault.
        assert!(limiter_warning(true, false, "none", "", (None, None)).is_none());
        assert!(
            limiter_warning(true, false, "none", "RTSS did not confirm", (None, None))
                .unwrap()
                .contains("(RTSS did not confirm)")
        );
        assert!(
            !limiter_warning(true, true, "none", "", (None, None))
                .unwrap()
                .contains("()")
        );
        assert!(limiter_warning(true, true, "rtss", "", (None, None)).is_none());
        assert!(
            limiter_warning(
                true,
                true,
                "rtss",
                "",
                (Some(Rate(116_000)), Some(Rate(60_000)))
            )
            .unwrap()
            .contains("60 fps")
        );
        assert!(limiter_warning(false, true, "rtss", "", (None, Some(Rate(60_000)))).is_some());
        assert!(
            limiter_warning(
                true,
                true,
                "rtss",
                "",
                (Some(Rate(60_000)), Some(Rate(60_000)))
            )
            .is_none()
        );
    }

    /// RL56's 3.4.0: "RTSS did not confirm restoration of LimitDenominator"
    /// at every stream end and start, the limiter held for good.
    #[test]
    fn a_restoration_rtss_states_otherwise_or_cannot_report_is_confirmed() {
        let restored = |wanted: &_, reported: &_| confirmed(wanted, reported, false);
        let map = |values: &[(&str, Option<u32>)]| -> BTreeMap<String, Option<u32>> {
            values.iter().map(|(k, v)| (k.to_string(), *v)).collect()
        };
        let all = |limit, denominator, sync| {
            map(&[
                ("Limit", limit),
                ("LimitDenominator", denominator),
                ("SyncLimiter", sync),
            ])
        };
        let wanted = all(Some(0), Some(1000), Some(0));
        // No limit: any denominator is no cap.
        assert_eq!(
            restored(&wanted, &all(Some(0), Some(1), Some(0))),
            Ok(vec![])
        );
        // The same cap stated another way, a zero denominator as 1.
        let wanted = all(Some(2997), Some(25), Some(1));
        assert_eq!(
            restored(&wanted, &all(Some(11988), Some(100), Some(1))),
            Ok(vec![])
        );
        assert_eq!(
            restored(
                &all(Some(60), Some(0), None),
                &all(Some(60), Some(1), Some(1))
            ),
            Ok(vec![])
        );
        // A key this RTSS doesn't report is named, not pending, but never
        // the limit itself.
        assert_eq!(
            restored(&wanted, &all(Some(2997), None, Some(1))),
            Ok(vec!["LimitDenominator".to_string()])
        );
        assert_eq!(
            restored(&wanted, &all(None, None, None)),
            Err("Limit".to_string())
        );
        // Another cap, or another sync mode, is a failed restoration.
        assert_eq!(
            restored(&wanted, &all(Some(2997), Some(1), Some(1))),
            Err("LimitDenominator".to_string())
        );
        assert_eq!(
            restored(
                &all(Some(0), Some(1), Some(1)),
                &all(Some(120), Some(1), Some(1))
            ),
            Err("Limit".to_string())
        );
        assert_eq!(
            restored(&wanted, &all(Some(2997), Some(25), Some(0))),
            Err("SyncLimiter".to_string())
        );
        // Only the denominator to restore: the user's later limit stays.
        assert_eq!(
            restored(
                &map(&[("LimitDenominator", Some(1))]),
                &all(Some(0), Some(1000), Some(1))
            ),
            Ok(vec![])
        );
        assert_eq!(
            restored(
                &map(&[("LimitDenominator", Some(1))]),
                &all(Some(90), Some(1000), Some(1))
            ),
            Err("LimitDenominator".to_string())
        );
    }

    /// The coordinator's review of the fix above: a stream's cap RTSS
    /// doesn't report back is never "applied and verified".
    #[test]
    fn a_stream_cap_rtss_cannot_report_is_not_confirmed() {
        let all = |limit, denominator, sync| -> BTreeMap<String, Option<u32>> {
            [
                ("Limit", limit),
                ("LimitDenominator", denominator),
                ("SyncLimiter", sync),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
        };
        let applied = |wanted: &_, reported: &_| confirmed(wanted, reported, true);
        let fractional = all(Some(2997), Some(25), Some(1));
        // Nothing reported: nothing confirmed.
        assert_eq!(
            applied(&fractional, &all(None, None, None)),
            Err("Limit".to_string())
        );
        // 2997 without its denominator could be 2997 fps.
        assert_eq!(
            applied(&fractional, &all(Some(2997), None, Some(1))),
            Err("LimitDenominator".to_string())
        );
        // A whole cap needs no denominator; what isn't reported is named, and
        // the console says so.
        let whole = all(Some(60), Some(1), Some(1));
        assert_eq!(
            applied(&whole, &all(Some(60), None, None)),
            Ok(vec![
                "LimitDenominator".to_string(),
                "SyncLimiter".to_string()
            ])
        );
        assert_eq!(applied(&whole, &whole), Ok(vec![]));
        let message = unconfirmed_message(&["SyncLimiter".to_string()]);
        assert!(message.contains("SyncLimiter"), "{message}");
        let warning = limiter_warning(true, true, "rtss", &message, (None, None)).unwrap();
        assert!(warning.contains("not fully applied"), "{warning}");
    }
}
