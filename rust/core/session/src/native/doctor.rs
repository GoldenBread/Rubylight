//! Stream Doctor 2.0 (`GET /stream/doctor`, `POST /stream/doctor/apply`): the
//! host's stage attribution and encoder facts as the wire types carry them,
//! and the session-only remedies the host applies and undoes.
//!
//! A remedy is an offer the player accepts, for this stream only: nothing here
//! touches the saved configuration, and a stream's trials end with it. Undo
//! goes back to the value from before the trial only while the trial's value
//! is still the one in force; a manual or automatic change since then wins.
use butterpollo_media_policy::pipeline_rates::PipelineRates as Rates;
use butterpollo_media_policy::stutter_report::{Report, Stage};
use rubylight_native_api::doctor::{
    ActiveTrial, DoctorSnapshot, EligibleRemedy, EncoderFacts, HostStage, HostStages,
    PipelineRates, RemedyId, SCHEMA_VERSION, StageTimes, TRIAL_FLOOR_KBPS, WINDOW_SECONDS,
};
use rubylight_native_api::error::ErrorCode;

/// The wire's name of a stutter report stage.
pub fn host_stage(stage: Stage) -> HostStage {
    match stage {
        Stage::Capture => HostStage::Capture,
        Stage::Convert => HostStage::Convert,
        Stage::Encode => HostStage::Encode,
        Stage::Packetize => HostStage::Packetize,
        Stage::Send => HostStage::Send,
    }
}

/// The window's stage attribution, `None` before a picture was measured.
pub fn stages(report: &Report, rates: Option<&Rates>) -> Option<HostStages> {
    (report.pictures() > 0).then(|| HostStages {
        pictures: report.pictures(),
        late_pictures: report.late_pictures(),
        dominant_stage: report.dominant().map(host_stage),
        lost_us: StageTimes {
            capture: report.lost_us(Stage::Capture),
            convert: report.lost_us(Stage::Convert),
            encode: report.lost_us(Stage::Encode),
            packetize: report.lost_us(Stage::Packetize),
            send: report.lost_us(Stage::Send),
        },
        pipeline_rates: rates.map(|rates| PipelineRates {
            source_mhz: rates.source_mhz,
            capture_mhz: rates.capture_mhz,
            submit_mhz: rates.submit_mhz,
            output_mhz: rates.output_mhz,
            repeat_mhz: rates.repeat_mhz,
        }),
    })
}

/// Why the host does not apply or undo a remedy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The caller has no stream of its own on this host.
    NoSession,
    /// The caller is a guest: only the device playing changes the stream.
    NotOwner,
    /// The encoder runs at [`TRIAL_FLOOR_KBPS`] or below.
    AtMinimum,
    /// A step-down is already being tried.
    TrialActive,
    /// The host does not know the encoder's live bitrate.
    BitrateUnknown,
    /// No trial of that remedy and ID is in force.
    NoTrial,
    /// Something else changed the bitrate since the trial; undo would undo that.
    Replaced,
    /// A remedy this host does not apply.
    UnknownRemedy,
}

impl Refusal {
    /// The stable name in [`EligibleRemedy::reason`] and error messages.
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::NoSession => "no_session",
            Refusal::NotOwner => "not_owner",
            Refusal::AtMinimum => "at_minimum",
            Refusal::TrialActive => "trial_active",
            Refusal::BitrateUnknown => "bitrate_unknown",
            Refusal::NoTrial => "no_trial",
            Refusal::Replaced => "trial_replaced",
            Refusal::UnknownRemedy => "unknown_remedy",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Refusal::NoSession => "This device has no stream on this PC.",
            Refusal::NotOwner => "Only the device playing the game changes the stream.",
            Refusal::AtMinimum => "The encoder is already at the lowest bitrate a trial uses.",
            Refusal::TrialActive => "A lower bitrate is already being tried. Undo it first.",
            Refusal::BitrateUnknown => "The PC does not know the encoder's current bitrate yet.",
            Refusal::NoTrial => "There is no such trial to undo on this stream.",
            Refusal::Replaced => {
                "The bitrate changed since the trial, so undo was cancelled to keep that change."
            }
            Refusal::UnknownRemedy => "This PC does not offer that remedy.",
        }
    }

    pub fn code(self) -> ErrorCode {
        match self {
            Refusal::NoSession => ErrorCode::NoSession,
            Refusal::NotOwner => ErrorCode::NotSessionOwner,
            Refusal::UnknownRemedy | Refusal::NoTrial => ErrorCode::InvalidValue,
            Refusal::AtMinimum
            | Refusal::TrialActive
            | Refusal::BitrateUnknown
            | Refusal::Replaced => ErrorCode::NotWritable,
        }
    }
}

/// The bitrate a step-down asks for: a fifth lower, not below the floor.
pub fn stepdown_kbps(live_kbps: u32) -> u32 {
    (u64::from(live_kbps) * 4 / 5).max(u64::from(TRIAL_FLOOR_KBPS)) as u32
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BitrateTrial {
    id: String,
    previous_kbps: u32,
    applied_kbps: u32,
}

/// A stream's trials. It lives in the session, so they end with the stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trials {
    issued: u32,
    bitrate: Option<BitrateTrial>,
}

impl Trials {
    /// The trial while its bitrate is still the one in force.
    fn live_trial(&self, live_kbps: u32) -> Option<&BitrateTrial> {
        self.bitrate
            .as_ref()
            .filter(|trial| trial.applied_kbps == live_kbps)
    }

    pub fn active(&self, live_kbps: u32) -> Vec<ActiveTrial> {
        self.bitrate
            .iter()
            .map(|trial| ActiveTrial {
                trial_id: trial.id.clone(),
                remedy: RemedyId::BitrateStepdown,
                applied_value: trial.applied_kbps,
                previous_value: trial.previous_kbps,
                can_undo: trial.applied_kbps == live_kbps,
            })
            .collect()
    }

    /// What a step-down would ask for, or why not.
    pub fn check_apply(
        &self,
        owner: bool,
        remedy: RemedyId,
        live_kbps: u32,
    ) -> Result<u32, Refusal> {
        if remedy != RemedyId::BitrateStepdown {
            return Err(Refusal::UnknownRemedy);
        }
        if !owner {
            return Err(Refusal::NotOwner);
        }
        if live_kbps == 0 {
            return Err(Refusal::BitrateUnknown);
        }
        if self.live_trial(live_kbps).is_some() {
            return Err(Refusal::TrialActive);
        }
        if live_kbps <= TRIAL_FLOOR_KBPS {
            return Err(Refusal::AtMinimum);
        }
        Ok(stepdown_kbps(live_kbps))
    }

    pub fn eligible(&self, owner: bool, live_kbps: u32) -> Vec<EligibleRemedy> {
        let remedy = match self.check_apply(owner, RemedyId::BitrateStepdown, live_kbps) {
            Ok(target) => EligibleRemedy {
                id: RemedyId::BitrateStepdown,
                configurable: true,
                target_kbps: Some(target),
                reason: None,
                message: None,
            },
            Err(refusal) => EligibleRemedy {
                id: RemedyId::BitrateStepdown,
                configurable: false,
                target_kbps: None,
                reason: Some(refusal.reason().into()),
                message: Some(refusal.message().into()),
            },
        };
        vec![remedy]
    }

    /// Records the step-down the host applied, from `previous_kbps`; a trial
    /// something else replaced is forgotten. Returns its ID.
    pub fn applied(&mut self, previous_kbps: u32, applied_kbps: u32) -> String {
        self.issued += 1;
        let id = format!("t-{}", self.issued);
        self.bitrate = Some(BitrateTrial {
            id: id.clone(),
            previous_kbps,
            applied_kbps,
        });
        id
    }

    /// The bitrate an undo returns to. A trial something else replaced is
    /// dropped and refused, so the change that replaced it stays.
    pub fn begin_undo(
        &mut self,
        owner: bool,
        remedy: RemedyId,
        trial_id: Option<&str>,
        live_kbps: u32,
    ) -> Result<u32, Refusal> {
        if remedy != RemedyId::BitrateStepdown {
            return Err(Refusal::UnknownRemedy);
        }
        if !owner {
            return Err(Refusal::NotOwner);
        }
        let trial = self
            .bitrate
            .as_ref()
            .filter(|trial| trial_id.is_none_or(|id| id == trial.id))
            .ok_or(Refusal::NoTrial)?;
        if trial.applied_kbps != live_kbps {
            self.bitrate = None;
            return Err(Refusal::Replaced);
        }
        Ok(trial.previous_kbps)
    }

    /// The undo went through.
    pub fn undone(&mut self) {
        self.bitrate = None;
    }
}

/// What the host knows about the caller's stream.
pub struct Facts {
    pub session_active: bool,
    pub owner: bool,
    pub encoder: Option<EncoderFacts>,
    pub stages: Option<HostStages>,
}

/// `GET /stream/doctor`'s answer.
pub fn snapshot(facts: Facts, trials: &Trials) -> DoctorSnapshot {
    let live = facts
        .encoder
        .as_ref()
        .map_or(0, |encoder| encoder.live_bitrate_kbps);
    let streaming = facts.session_active && facts.encoder.is_some();
    DoctorSnapshot {
        schema_version: SCHEMA_VERSION,
        session_active: facts.session_active,
        caller_is_owner: facts.owner,
        window_seconds: WINDOW_SECONDS,
        encoder: facts.encoder,
        host_stages: facts.stages,
        active_trials: if streaming {
            trials.active(live)
        } else {
            Vec::new()
        },
        eligible_remedies: if streaming {
            trials.eligible(facts.owner, live)
        } else {
            Vec::new()
        },
    }
}

#[cfg(test)]
mod tests;
