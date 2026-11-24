//! Shared non-UI connection lifecycle classification: termination routing,
//! network loss/return recovery gating, automatic rejoin, and audio config.

use nightjar_policy::continue_here;
use nightjar_policy::http::{AppControl, AppRole};
use nightjar_policy::launch;
use nightjar_policy::recovery;

use crate::http::{Error as HttpError, asks_to_confirm_replace};

use crate::sdp::AudioConfiguration;
use crate::session::{NetworkChange, error};

/// UI-level action following a stream termination evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminationUiAction {
    Finish,
    ShowMovedPrompt,
    TryAutoRecover,
    ShowDialog,
    ShowToastAndFinish,
}

/// Complete non-UI classification of a stream termination event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminationEvaluation {
    pub effective_code: i32,
    pub may_have_moved: bool,
    pub recovery_follows: bool,
    pub transient_failure: bool,
    pub skip_port_test: bool,
    pub keep_recovery_media_failure: bool,
    pub needs_action_dialog: bool,
}

/// Evaluates `connectionTerminated` flags before running port tests / UI dialogs.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_termination(
    reported_code: i32,
    exit_requested: bool,
    host_gone: bool,
    ended_session: bool,
    moved_to_present: bool,
    guest_ended: bool,
    video_started: bool,
    auto_reconnect: bool,
) -> TerminationEvaluation {
    let effective_code = if host_gone {
        error::HOST_GONE
    } else if exit_requested || ended_session {
        error::GRACEFUL
    } else {
        reported_code
    };
    let may_have_moved = !host_gone
        && !exit_requested
        && (reported_code == error::GRACEFUL || recovery::transient_termination(reported_code));
    let recovers = recovery::recovers_termination(effective_code, video_started);
    let recovery_follows = auto_reconnect && recovers;
    let transient_failure = !guest_ended && continue_here::recovers(recovers, moved_to_present);
    let skip_port_test = effective_code == error::GRACEFUL
        || host_gone
        || effective_code == -106 // CLIENT_ERROR_VULKAN_RENDERER
        || effective_code == launch::CLIENT_ERROR_DECODER
        || (transient_failure && auto_reconnect);
    let keep_recovery_media_failure =
        transient_failure && !recovery::transport_termination(effective_code);
    TerminationEvaluation {
        effective_code,
        may_have_moved,
        recovery_follows,
        transient_failure,
        skip_port_test,
        keep_recovery_media_failure,
        needs_action_dialog: false,
    }
}

/// Finalizes whether the foreground UI should show an action dialog or a toast and finish.
pub fn termination_needs_action_dialog(
    effective_code: i32,
    has_host_failure: bool,
    has_resume_intent: bool,
    port_test_blocked: bool,
    grace_gave_up: bool,
    recovery_exhausted: bool,
) -> bool {
    has_resume_intent
        || has_host_failure
        || launch::termination_needs_action(
            effective_code,
            port_test_blocked,
            grace_gave_up,
            recovery_exhausted,
        )
}

/// Whether the lost-network overlay promises an automatic reconnect.
pub fn network_lost_promises_reconnect(
    auto_recovers_on_its_own: bool,
    network_grace_ms: u32,
) -> bool {
    auto_recovers_on_its_own && network_grace_ms <= recovery::PEER_TIMEOUT_MS
}

/// Whether returning to a network triggers an immediate reconnect.
pub fn reconnects_on_network_return(
    change: NetworkChange,
    metered: bool,
    roaming_metered: bool,
    auto_reconnect: bool,
    network_grace_ms: u32,
    on_its_own: bool,
) -> bool {
    on_its_own
        && recovery::reconnects_at_once(
            change == NetworkChange::CannotContinue,
            metered,
            roaming_metered,
            auto_reconnect,
            network_grace_ms,
        )
}

/// Whether a dropped stream of `role` reconnects automatically.
pub fn reconnects_automatically(auto_reconnect: bool, away: bool, role: AppRole) -> bool {
    auto_reconnect && !away && role == AppRole::Stream
}

/// Whether a refused launch asks the user to confirm replacing the running app.
pub fn asks_replace(resume: bool, confirm_replace: bool, role: AppRole, error: &HttpError) -> bool {
    !resume && !confirm_replace && role == AppRole::Stream && asks_to_confirm_replace(error)
}

/// Audio configuration requested for a stream, falling back from 7.1.4 to 7.1
/// unless both the host and local output support 12 channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestedAudioChannels {
    Stereo,
    Surround51,
    Surround71,
    Surround714,
}

pub fn audio_configuration(
    choice: RequestedAudioChannels,
    host_714: bool,
    output_714: bool,
) -> AudioConfiguration {
    match choice {
        RequestedAudioChannels::Stereo => AudioConfiguration::STEREO,
        RequestedAudioChannels::Surround51 => AudioConfiguration::SURROUND_51,
        RequestedAudioChannels::Surround71 => AudioConfiguration::SURROUND_71,
        RequestedAudioChannels::Surround714 if host_714 && output_714 => {
            AudioConfiguration::SURROUND_714
        }
        RequestedAudioChannels::Surround714 => {
            let why = if host_714 {
                "this output doesn't play 12 channels"
            } else {
                "the PC doesn't report 7.1.4"
            };
            eprintln!("nightjar: audio: {why}; asking for 7.1");
            AudioConfiguration::SURROUND_71
        }
    }
}

/// Whether a control tile launch needs to resolve running-game ID on resume.
pub fn should_rejoin_control_as_running(role: AppRole, control: AppControl) -> bool {
    role == AppRole::Stream && control != AppControl::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termination_evaluation_handles_host_gone_and_guest_ended() {
        let eval = evaluate_termination(-105, false, true, false, false, false, true, true);
        assert_eq!(eval.effective_code, error::HOST_GONE);
        assert!(!eval.transient_failure);
        assert!(eval.skip_port_test);

        let guest = evaluate_termination(-102, false, false, false, false, true, true, true);
        assert!(!guest.transient_failure);
    }

    #[test]
    fn network_return_reconnect_respects_grace_and_metered() {
        assert!(reconnects_on_network_return(
            NetworkChange::CannotContinue,
            false,
            false,
            true,
            3000,
            true
        ));
        assert!(!reconnects_on_network_return(
            NetworkChange::Roamed,
            false,
            false,
            true,
            3000,
            true
        ));
        assert!(!reconnects_on_network_return(
            NetworkChange::CannotContinue,
            true,
            false,
            true,
            3000,
            true
        ));
        assert!(reconnects_on_network_return(
            NetworkChange::CannotContinue,
            true,
            true,
            true,
            3000,
            true
        ));
    }
}
