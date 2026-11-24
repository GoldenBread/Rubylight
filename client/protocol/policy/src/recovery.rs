//! Automatic reconnecting: which failures may retry, and the retry budget.
//!
//! [`StreamRecoveryPolicy`] is Android's (`StreamRecoveryPolicy.java`). [`Backoff`] is the
//! desktop client's budget, which starts its window at the drop and resets on a stable
//! connection's length instead of its start time.

use serde_json::Value;

use crate::trace::{self, Args, Subject};

pub const MAX_ATTEMPTS: u32 = 5;
pub const WINDOW_MS: u64 = 60_000;
pub const STABLE_MS: u64 = 30_000;
/// The resume after the user's own trip: the first retry's delay, never counted as an attempt.
pub const TRIP_RESUME_DELAY_MS: u64 = 1_000;
const DELAYS_MS: [u64; MAX_ATTEMPTS as usize] = [1000, 2000, 4000, 8000, 12000];

/// How an automatic recovery that gives up ends, after why its last attempt failed.
pub const RECOVERY_STOPPED: &str =
    "Automatic reconnecting stopped. Your game was not quit; check the connection and try again.";
/// Why a recovery gave up when no attempt got as far as failing: the PC never answered within its minute.
pub const RECOVERY_UNREACHABLE: &str = "The PC could not be reached.";

/// A retry budget that survives replacing the stream activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamRecoveryPolicy {
    attempts: u32,
    started_at_ms: u64,
}

impl StreamRecoveryPolicy {
    /// Restores saved state; an out-of-range attempt count is clamped.
    pub fn new(attempts: u32, started_at_ms: u64) -> Self {
        Self {
            attempts: attempts.min(MAX_ATTEMPTS),
            started_at_ms,
        }
    }

    /// The delay before the next attempt, or `None` when the budget is spent.
    pub fn next_delay(&mut self, now_ms: u64, connected_at_ms: Option<u64>) -> Option<u64> {
        self.reset_after_stable_connection(now_ms, connected_at_ms);
        if self.attempts == 0 {
            self.started_at_ms = now_ms;
        }
        if self.attempts >= MAX_ATTEMPTS || now_ms < self.started_at_ms {
            return None;
        }
        let delay = DELAYS_MS[self.attempts as usize];
        if now_ms - self.started_at_ms + delay >= WINDOW_MS {
            return None;
        }
        self.attempts += 1;
        Some(delay)
    }

    /// Whether [`StreamRecoveryPolicy::next_delay`] would allow an attempt now, spending none.
    pub fn allows_attempt(&self, now_ms: u64, connected_at_ms: Option<u64>) -> bool {
        let mut probe = *self;
        probe.next_delay(now_ms, connected_at_ms).is_some()
    }

    pub fn reset_after_stable_connection(&mut self, now_ms: u64, connected_at_ms: Option<u64>) {
        // Briefly starting a stream must not give a flapping connection an unlimited budget.
        if connected_at_ms.is_some_and(|at| now_ms.saturating_sub(at) >= STABLE_MS) {
            *self = Self::default();
        }
    }

    /// The budget left after the user's own trip away (Home, Files, a lock) that began at
    /// `left_at_ms`: the trip spends nothing. With no failure being recovered the budget is full;
    /// a window already running pauses for the time away instead of running on.
    pub fn after_user_trip(
        attempts: u32,
        started_at_ms: u64,
        left_at_ms: u64,
        now_ms: u64,
    ) -> Self {
        if attempts == 0 {
            return Self::default();
        }
        Self::new(
            attempts,
            started_at_ms.saturating_add(now_ms.saturating_sub(left_at_ms)),
        )
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }
}

/// How long the stream had been connected before a user trip, kept only if a recovery budget was
/// already running.
pub fn trip_connected_before(attempts: u32, connected_for_ms: u64) -> u64 {
    if attempts > 0 { connected_for_ms } else { 0 }
}

/// The cause a recovery that gives up names: the last failure's, else `unreachable`.
pub fn gave_up_cause<'a>(last_failure: Option<&'a str>, unreachable: &'a str) -> &'a str {
    last_failure
        .filter(|m| !m.is_empty())
        .unwrap_or(unreachable)
}

/// What an automatic recovery that gives up says: why its last attempt failed, then that it
/// stopped and the game was not quit.
pub fn gave_up_text(cause: &str, stopped: &str) -> String {
    if cause.is_empty() {
        stopped.to_owned()
    } else {
        format!("{cause}\n\n{stopped}")
    }
}

/// Combined give-up text for both Desktop and Android.
pub fn recovery_gave_up(last_failure: Option<&str>, unreachable: &str, stopped: &str) -> String {
    gave_up_text(gave_up_cause(last_failure, unreachable), stopped)
}

/// Where automatic bitrate starts after a reconnect: on an `automatic` reconnect of a stream whose
/// automatic bitrate owned the rate, the rate it had cut to; `None` after a bitrate the player set.
pub fn reconnect_bitrate_start(
    automatic: bool,
    automatic_owned: bool,
    manual: bool,
    kbps: u32,
) -> Option<u32> {
    (automatic && automatic_owned && !manual && kbps > 0).then_some(kbps)
}

/// Where automatic bitrate's first claim starts: the encoder's rate, or an earlier cut when lower.
pub fn automatic_bitrate_start(encoder_kbps: u32, resume_kbps: Option<u32>) -> u32 {
    resume_kbps
        .filter(|&kbps| kbps > 0 && kbps < encoder_kbps)
        .unwrap_or(encoder_kbps)
}

/// Whether video started: this connection showed a picture, or an earlier one of its automatic
/// reconnect chain did.
pub fn video_started(decoder_received_video: bool, chain_video_shown: bool) -> bool {
    decoder_received_video || chain_video_shown
}

/// Whether this stream start is a recovery attempt or a resume after a user trip.
pub fn is_recovery_attempt(attempts: u32, trip_resume: bool) -> bool {
    attempts > 0 || trip_resume
}

/// Whether this start only resumes an existing game (never launches a new one).
pub fn is_resume_only_stream(attempts: u32, trip_resume: bool, output_resume_only: bool) -> bool {
    is_recovery_attempt(attempts, trip_resume) || output_resume_only
}

/// Whether handing off starts the next screen in this stream's own task before finishing.
pub fn hand_off_in_place(task_root: bool, multi_window: bool, picture_in_picture: bool) -> bool {
    task_root && multi_window && !picture_in_picture
}

/// Whether a lost connection would reconnect automatically right now (spends nothing).
#[allow(clippy::too_many_arguments)]
pub fn can_auto_recover(
    auto_reconnect: bool,
    displayed_failure: bool,
    connected: bool,
    foreground: bool,
    video_started: bool,
    finishing_or_destroyed: bool,
    handoff_or_pending: bool,
    stream_role: bool,
    control_action: bool,
    has_resume_target: bool,
    policy: StreamRecoveryPolicy,
    now_ms: u64,
    connected_at_ms: Option<u64>,
) -> bool {
    auto_reconnect
        && !displayed_failure
        && connected
        && foreground
        && video_started
        && !finishing_or_destroyed
        && !handoff_or_pending
        && stream_role
        && !control_action
        && has_resume_target
        && policy.allows_attempt(now_ms, connected_at_ms)
}

/// Whether leaving the stream should keep a resume intent for when the user returns.
#[allow(clippy::too_many_arguments)]
pub fn can_remember_resume(
    already_remembered: bool,
    auto_reconnect: bool,
    connected: bool,
    displayed_failure: bool,
    finishing_or_destroyed: bool,
    cancelled_on_pause: bool,
    handoff_in_progress: bool,
    recovery_app_id: i32,
    stream_role: bool,
    control_action: bool,
) -> bool {
    !already_remembered
        && auto_reconnect
        && connected
        && !displayed_failure
        && !finishing_or_destroyed
        && !cancelled_on_pause
        && !handoff_in_progress
        && recovery_app_id != 0
        && stream_role
        && !control_action
}

/// Native media timeouts and transport errors. Graceful (0), protected content (-103),
/// conversion (-104) and unrecognized host errors never retry.
pub fn transient_termination(code: i32) -> bool {
    matches!(code, -102..=-100) || transport_termination(code)
}

/// Whether a stream that ended with `code` reconnects on its own: a [`transient_termination`]
/// of a connection whose video started (its first frame arrived). One that never had video
/// failed to start, most often at a firewall blocking the media ports: reconnecting would only
/// hide why behind up to [`MAX_ATTEMPTS`] attempts, a minute in all. Ported from Moonlight V+
/// PC (qiin2333/moonlight-qt 6792aeed, "avoid reconnect loop before first video").
pub fn recovers_termination(code: i32, video_started: bool) -> bool {
    video_started && transient_termination(code)
}

/// Only ambiguous transport failures can stand in for a lost host-exit packet. Media timeouts
/// and an early host exit keep their meaning. -1 is an ENet failure without an errno.
pub fn transport_termination(code: i32) -> bool {
    matches!(code, -1 | 32 | 54 | 57 | 60 | 64 | 100..=104 | 107 | 110..=113
        | 10050..=10054 | 10057 | 10060 | 10061 | 10064 | 10065)
}

/// The start stages whose transport failures Android retries during a recovery, by
/// `LiGetStageName` (Android's `StreamRecoveryPolicy.RETRYABLE_STAGES`, mirrored here for the
/// traces). The desktop's table is `RETRYABLE_START_STAGES` in its `stream/recovery.rs`, and
/// lists the same stages. Video and audio stages stay out: a decoder or audio device that fails
/// reports codes a transport failure uses too.
pub const RETRYABLE_STAGES: [&str; 5] = [
    "name resolution",
    "RTSP handshake",
    "control stream initialization",
    "control stream establishment",
    "input stream establishment",
];

/// Whether a failed start may retry while recovering a previously connected session. Host
/// rejection, pairing, permission and decoder errors need a user decision.
pub fn transient_startup(stage: Option<&str>, port_flags: i32, error: i32) -> bool {
    // NvConnection reports Java transport failures with both HTTP port flags and error 0; its
    // stage is the game's name then, not a native startup stage.
    if port_flags == 0x0001 | 0x0002 && error == 0 {
        return true;
    }
    let Some(stage) = stage else {
        return false;
    };
    // Native startup reports positive errno values; positive host status codes stay manual, so
    // only known transport errors pass. Only RTSP and control startup carry port flags.
    (error <= 0 || transient_termination(error))
        && (RETRYABLE_STAGES
            .iter()
            .any(|s| stage.eq_ignore_ascii_case(s))
            || (port_flags != 0 && !stage.to_lowercase().contains("video")))
}

/// Between two route checks while the PC is not reachable (Android's `NetworkReturnRecovery`).
pub const ROUTE_RETRY_MS: u64 = 500;
/// One route check's connect timeout: below [`ROUTE_RETRY_MS`], so checks keep their pace.
pub const ROUTE_PROBE_TIMEOUT_MS: u64 = 400;

/// The connection's own timeout without a network drop tolerance (the engine's
/// `CONTROL_PEER_TIMEOUT`): a tolerance up to it changes nothing.
pub const PEER_TIMEOUT_MS: u32 = 10_000;

/// "Wait through network dropouts": the waits both clients offer, in seconds. 0 is Off: the
/// connection keeps its own timeout ([`PEER_TIMEOUT_MS`]), so any wait up to it changes nothing
/// and reads as Off. Android stores the seconds (`list_network_grace`), desktop the milliseconds
/// (`network_grace_ms`); neither default changes.
pub const NETWORK_GRACE_CHOICES_S: [u32; 5] = [0, 15, 20, 30, 60];

/// The choice, in seconds, that a stored wait in milliseconds reads as: Off up to the
/// connection's own timeout, else the nearest longer wait offered (a tie takes the longer one).
pub fn network_grace_choice_s(stored_ms: u32) -> u32 {
    if stored_ms <= PEER_TIMEOUT_MS {
        return 0;
    }
    NETWORK_GRACE_CHOICES_S
        .into_iter()
        .filter(|&s| s > 0)
        .min_by_key(|&s| {
            (u64::from(s) * 1000).abs_diff(u64::from(stored_ms)) * 2
                + u64::from(s * 1000 < stored_ms)
        })
        .unwrap_or(0)
}

/// A stored wait as the engine takes it: Off as stored (any wait up to the timeout is the same),
/// a longer one moved onto its choice.
pub fn network_grace_ms(stored_ms: u32) -> u32 {
    match network_grace_choice_s(stored_ms) {
        0 => stored_ms,
        seconds => seconds * 1000,
    }
}

/// A network is back after the stream's was lost. True: reconnect now, since the engine said
/// the connection cannot carry on over it (`cannot_continue`: the route toward the PC leaves
/// from another address and the stream did not roam there) and would only time out. Never when
/// it roamed or carries on (the same address back, or no route yet: its own timeout decides),
/// with automatic reconnection off, on a `metered` network without "Continue on mobile data"
/// (`mobile_allowed`), or with a network drop tolerance above [`PEER_TIMEOUT_MS`] (it holds
/// the connection, and decides).
pub fn reconnects_at_once(
    cannot_continue: bool,
    metered: bool,
    mobile_allowed: bool,
    auto_reconnect: bool,
    network_grace_ms: u32,
) -> bool {
    cannot_continue
        && (!metered || mobile_allowed)
        && auto_reconnect
        && network_grace_ms <= PEER_TIMEOUT_MS
}

/// The wait for a route to the PC before a recovery attempt. A Wi-Fi network is often back
/// before it has an address toward the PC (DHCP after the reconnect), so the attempt waits,
/// checking every [`ROUTE_RETRY_MS`], instead of failing at once and backing off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteWait {
    deadline_ms: u64,
    unreachable: u32,
}

impl RouteWait {
    /// `deadline_ms`: when the recovery gives up (its window ends).
    pub fn new(deadline_ms: u64) -> Self {
        Self {
            deadline_ms,
            unreachable: 0,
        }
    }

    /// After a route check at `now_ms`: `Some(0)` to start the attempt, `Some(delay)` before the
    /// next check, `None` when the window ended without a route (the recovery gives up, loudly).
    pub fn next(&mut self, now_ms: u64, reachable: bool) -> Option<u64> {
        if reachable {
            return Some(0);
        }
        self.unreachable += 1;
        if now_ms >= self.deadline_ms {
            return None;
        }
        Some(ROUTE_RETRY_MS.min(self.deadline_ms - now_ms))
    }

    /// Route checks that found the PC unreachable so far.
    pub fn unreachable_checks(&self) -> u32 {
        self.unreachable
    }
}

/// The desktop client's reconnect budget: the same delays within one minute of the first drop,
/// renewed only after a connection that lasted [`STABLE_MS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backoff {
    attempts: u32,
    started_at_ms: Option<u64>,
    /// The next attempt starts without its delay (a network came back); it still spends one.
    now: bool,
}

impl Backoff {
    /// Starts the window when the stream drops, before the first attempt.
    pub fn begin(&mut self, now_ms: u64) {
        self.started_at_ms.get_or_insert(now_ms);
    }

    pub fn reset_after(&mut self, connected_ms: u64) {
        if connected_ms >= STABLE_MS {
            *self = Self::default();
        }
    }

    pub fn remaining(&self, now_ms: u64) -> u64 {
        self.started_at_ms.map_or(WINDOW_MS, |started| {
            WINDOW_MS.saturating_sub(now_ms.saturating_sub(started))
        })
    }

    pub fn next_delay(&mut self, now_ms: u64) -> Option<u64> {
        let delay = *DELAYS_MS.get(self.attempts as usize)?;
        let delay = if self.now { 0 } else { delay };
        self.started_at_ms.get_or_insert(now_ms);
        if delay >= self.remaining(now_ms) {
            return None;
        }
        self.now = false;
        self.attempts += 1;
        Some(delay)
    }

    /// Whether [`Backoff::next_delay`] at `now_ms` would allow an attempt, spending none.
    pub fn allows_attempt(&self, now_ms: u64) -> bool {
        DELAYS_MS.get(self.attempts as usize).is_some_and(|delay| {
            let delay = if self.now { 0 } else { *delay };
            delay < self.remaining(now_ms)
        })
    }

    /// The next attempt starts at once: the network came back without the stream roaming.
    pub fn retry_now(&mut self) {
        self.now = true;
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

fn classify(method: &str, args: &Value) -> Option<Result<Value, String>> {
    let result = match method {
        "transient_termination" => Args::new(args, 1)
            .and_then(|a| a.int(0))
            .map(|code| transient_termination(code).into()),
        "recovers_termination" => {
            Args::new(args, 2).and_then(|a| Ok(recovers_termination(a.int(0)?, a.flag(1)?).into()))
        }
        "transport_termination" => Args::new(args, 1)
            .and_then(|a| a.int(0))
            .map(|code| transport_termination(code).into()),
        "transient_startup" => Args::new(args, 3)
            .and_then(|a| Ok(transient_startup(a.text(0)?, a.int(1)?, a.int(2)?).into())),
        "after_user_trip" => Args::new(args, 4).and_then(|a| {
            let p =
                StreamRecoveryPolicy::after_user_trip(a.int(0)?, a.int(1)?, a.int(2)?, a.int(3)?);
            Ok(Value::Array(vec![
                p.attempts().into(),
                p.started_at_ms().into(),
            ]))
        }),
        _ => return None,
    };
    Some(result)
}

struct TracedPolicy(Option<StreamRecoveryPolicy>);

pub(crate) fn traced_policy(args: &Value) -> Result<Box<dyn Subject>, String> {
    let policy = match args {
        Value::Null => None,
        _ => {
            let a = Args::new(args, 2)?;
            Some(StreamRecoveryPolicy::new(a.int(0)?, a.int(1)?))
        }
    };
    Ok(Box::new(TracedPolicy(policy)))
}

impl Subject for TracedPolicy {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        if let Some(result) = classify(method, args) {
            return result;
        }
        let policy = self.0.as_mut().ok_or("no instance (new is null)")?;
        match method {
            "next_delay" => {
                let a = Args::new(args, 2)?;
                let delay = policy.next_delay(a.int(0)?, a.optional_ms(1)?);
                Ok(trace::optional_ms(delay))
            }
            "reset_after_stable_connection" => {
                let a = Args::new(args, 2)?;
                policy.reset_after_stable_connection(a.int(0)?, a.optional_ms(1)?);
                Ok(Value::Null)
            }
            "allows_attempt" => {
                let a = Args::new(args, 2)?;
                Ok(policy.allows_attempt(a.int(0)?, a.optional_ms(1)?).into())
            }
            "attempts" => Args::new(args, 0).map(|_| policy.attempts().into()),
            "started_at" => Args::new(args, 0).map(|_| policy.started_at_ms().into()),
            _ => Err(format!("unknown method {method}")),
        }
    }
}

pub(crate) fn traced_backoff(args: &Value) -> Result<Box<dyn Subject>, String> {
    Args::new(args, 0)?;
    Ok(Box::new(Backoff::default()))
}

impl Subject for Backoff {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        match method {
            "begin" => {
                self.begin(Args::new(args, 1)?.int(0)?);
                Ok(Value::Null)
            }
            "reset_after" => {
                self.reset_after(Args::new(args, 1)?.int(0)?);
                Ok(Value::Null)
            }
            "remaining" => Ok(self.remaining(Args::new(args, 1)?.int(0)?).into()),
            "next_delay" => {
                let delay = self.next_delay(Args::new(args, 1)?.int(0)?);
                Ok(trace::optional_ms(delay))
            }
            "attempts" => Args::new(args, 0).map(|_| self.attempts().into()),
            "allows_attempt" => {
                Args::new(args, 1).and_then(|a| Ok(self.allows_attempt(a.int(0)?).into()))
            }
            "retry_now" => {
                Args::new(args, 0)?;
                self.retry_now();
                Ok(Value::Null)
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

/// Java's `NetworkReturnRecovery`: `new` is the deadline, or null for the static decision.
pub(crate) fn traced_route_wait(args: &Value) -> Result<Box<dyn Subject>, String> {
    let wait = match args {
        Value::Null => None,
        _ => Some(RouteWait::new(Args::new(args, 1)?.int(0)?)),
    };
    Ok(Box::new(TracedRouteWait(wait)))
}

struct TracedRouteWait(Option<RouteWait>);

impl Subject for TracedRouteWait {
    fn call(&mut self, method: &str, args: &Value) -> Result<Value, String> {
        if method == "reconnects_at_once" {
            let a = Args::new(args, 5)?;
            return Ok(reconnects_at_once(
                a.flag(0)?,
                a.flag(1)?,
                a.flag(2)?,
                a.flag(3)?,
                a.int(4)?,
            )
            .into());
        }
        let wait = self.0.as_mut().ok_or("no instance (new is null)")?;
        match method {
            "next" => {
                let a = Args::new(args, 2)?;
                Ok(trace::optional_ms(wait.next(a.int(0)?, a.flag(1)?)))
            }
            "unreachable_checks" => Args::new(args, 0).map(|_| wait.unreachable_checks().into()),
            _ => Err(format!("unknown method {method}")),
        }
    }
}

#[cfg(test)]
mod network_grace_tests {
    use super::*;

    #[test]
    fn every_stored_wait_reads_as_one_choice() {
        // Desktop's old list (0, 3, 5, 10, 15 s) and default (3 s): up to the timeout all are Off.
        for ms in [0, 3_000, 5_000, 10_000] {
            assert_eq!(network_grace_choice_s(ms), 0, "{ms}");
            assert_eq!(network_grace_ms(ms), ms, "{ms}: Off stays as stored");
        }
        assert_eq!(network_grace_choice_s(15_000), 15);
        // Android's list (Off, 20, 30, 60 s) keeps its values.
        for s in [20, 30, 60] {
            assert_eq!(network_grace_choice_s(s * 1000), s);
        }
        // A hand-edited value moves to the nearest; a tie takes the longer wait.
        assert_eq!(network_grace_choice_s(10_001), 15);
        assert_eq!(network_grace_choice_s(17_500), 20);
        assert_eq!(network_grace_choice_s(24_000), 20);
        assert_eq!(network_grace_choice_s(45_000), 60);
        assert_eq!(network_grace_choice_s(600_000), 60);
        assert_eq!(network_grace_ms(12_000), 15_000);
        // Every choice reads as itself.
        for s in NETWORK_GRACE_CHOICES_S {
            assert_eq!(network_grace_choice_s(s * 1000), s);
        }
    }

    #[test]
    fn backoff_allows_attempt_respects_retry_now() {
        let mut backoff = Backoff::default();
        backoff.begin(0);
        // 500 ms left in the 60 s window, below DELAYS_MS[0] (1000 ms).
        let near_end = WINDOW_MS - 500;
        assert!(!backoff.allows_attempt(near_end));
        backoff.retry_now();
        assert!(backoff.allows_attempt(near_end));
        assert!(!backoff.allows_attempt(WINDOW_MS));
        assert_eq!(backoff.next_delay(near_end), Some(0));
    }

    #[test]
    fn user_trip_spends_nothing_and_pauses_running_window() {
        let idle = StreamRecoveryPolicy::after_user_trip(0, 0, 5_000, 120_000);
        assert_eq!((idle.attempts(), idle.started_at_ms()), (0, 0));
        assert_eq!(trip_connected_before(0, 12_000), 0);

        let paused = StreamRecoveryPolicy::after_user_trip(2, 1_000, 10_000, 70_000);
        assert_eq!((paused.attempts(), paused.started_at_ms()), (2, 61_000));
        assert_eq!(trip_connected_before(2, 12_000), 12_000);
    }

    #[test]
    fn give_up_and_bitrate_helpers_match_both_clients() {
        assert_eq!(
            gave_up_cause(None, RECOVERY_UNREACHABLE),
            RECOVERY_UNREACHABLE
        );
        assert_eq!(
            gave_up_cause(Some(""), RECOVERY_UNREACHABLE),
            RECOVERY_UNREACHABLE
        );
        assert_eq!(
            gave_up_cause(Some("refused"), RECOVERY_UNREACHABLE),
            "refused"
        );
        assert_eq!(
            recovery_gave_up(None, RECOVERY_UNREACHABLE, RECOVERY_STOPPED),
            format!("{RECOVERY_UNREACHABLE}\n\n{RECOVERY_STOPPED}")
        );
        assert_eq!(gave_up_text("", RECOVERY_STOPPED), RECOVERY_STOPPED);
        assert_eq!(
            reconnect_bitrate_start(true, true, false, 12_000),
            Some(12_000)
        );
        assert_eq!(reconnect_bitrate_start(false, true, false, 12_000), None);
        assert_eq!(reconnect_bitrate_start(true, false, false, 12_000), None);
        assert_eq!(reconnect_bitrate_start(true, true, true, 12_000), None);
        assert_eq!(reconnect_bitrate_start(true, true, false, 0), None);
        assert_eq!(automatic_bitrate_start(30_000, Some(12_000)), 12_000);
        assert_eq!(automatic_bitrate_start(20_000, Some(30_000)), 20_000);
        assert_eq!(automatic_bitrate_start(30_000, None), 30_000);
        assert_eq!(automatic_bitrate_start(30_000, Some(0)), 30_000);
        assert!(hand_off_in_place(true, true, false));
        assert!(!hand_off_in_place(true, true, true));
        assert!(can_auto_recover(
            true,
            false,
            true,
            true,
            true,
            false,
            false,
            true,
            false,
            true,
            StreamRecoveryPolicy::default(),
            1_000,
            Some(0),
        ));
        assert!(!can_auto_recover(
            false,
            false,
            true,
            true,
            true,
            false,
            false,
            true,
            false,
            true,
            StreamRecoveryPolicy::default(),
            1_000,
            Some(0),
        ));
        assert!(can_remember_resume(
            false, true, true, false, false, false, false, 42, true, false
        ));
        assert!(!can_remember_resume(
            true, true, true, false, false, false, false, 42, true, false
        ));
    }
}
