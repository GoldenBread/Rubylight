//! `GET /stream/doctor` and `POST /stream/doctor/apply` (`docs/native-api.md`,
//! "Stream Doctor"): which host stage lost the caller's stream time over the
//! last 30 seconds, and the session-only remedies the host applies and undoes
//! for it. Reading needs View or Launch; a remedy needs Launch and the stream's
//! ownership. The rules live in `butterpollo_session::native::doctor`; this
//! gathers what the host already measures, on request only.
use super::{Failure, Reply, caller, json, read_body, require, require_view_or_launch};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_session::{
    native::{
        LAUNCH,
        doctor::{self, Facts, Refusal},
    },
    session::Role,
    state::Client,
};
use butterpollo_stream::{
    nvhttp,
    state::{Session, Shared},
    tls::Connection,
};
use rubylight_native_api::{
    doctor::{DoctorAction, DoctorApplyRequest, DoctorSnapshot, EncoderFacts, WINDOW_SECONDS},
    error::ErrorCode,
};
use std::{
    sync::{Arc, PoisonError, atomic::Ordering},
    time::{Duration, Instant},
};

/// `GET /stream/doctor`.
pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    let snapshot =
        tokio::task::spawn_blocking(move || snapshot(own_stream(&h, &client).as_deref()))
            .await
            .map_err(|error| Failure::internal("read the stream doctor", error))?;
    Ok(json(StatusCode::OK, &snapshot))
}

/// `POST /stream/doctor/apply`: applies or undoes one remedy, then answers
/// with the snapshot after it.
pub async fn apply(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require(&client, LAUNCH, "Launch applications")?;
    let request: DoctorApplyRequest =
        serde_json::from_slice(&read_body(request).await?).map_err(|error| {
            Failure::new(
                ErrorCode::InvalidRequest,
                format!("The body is not a Stream Doctor request: {error}."),
            )
        })?;
    let snapshot = tokio::task::spawn_blocking(move || run(&h, &client, &request))
        .await
        .map_err(|error| Failure::internal("apply the stream doctor's remedy", error))??;
    Ok(json(StatusCode::OK, &snapshot))
}

fn refused(refusal: Refusal) -> Failure {
    Failure::new(refusal.code(), refusal.message())
}

fn run(
    h: &Shared,
    client: &Client,
    request: &DoctorApplyRequest,
) -> Result<DoctorSnapshot, Failure> {
    let s = own_stream(h, client).ok_or_else(|| refused(Refusal::NoSession))?;
    let owner = owns(&s);
    let mut trials = s.doctor.lock().unwrap_or_else(PoisonError::into_inner);
    let live = s.bitrate.load(Ordering::Acquire);
    let (target, undo) = match request.action {
        DoctorAction::Apply => (trials.check_apply(owner, request.remedy, live), false),
        DoctorAction::Undo => (
            trials.begin_undo(owner, request.remedy, request.trial_id.as_deref(), live),
            true,
        ),
    };
    let target = target.map_err(refused)?;
    let applied = butterpollo_media_policy::stream_policy::runtime_bitrate_kbps(
        &h.config.read().unwrap_or_else(PoisonError::into_inner),
        target,
    );
    nvhttp::set_client_bitrate(&s, target, applied);
    if undo {
        trials.undone();
    } else {
        trials.applied(live, applied);
    }
    tracing::info!(
        client = %client.name,
        undo,
        from_kbps = live,
        to_kbps = applied,
        "stream doctor: bitrate step-down {}",
        if undo { "undone" } else { "applied" }
    );
    drop(trials);
    Ok(snapshot(Some(&s)))
}

/// The caller's live stream, a Stream before a monitor or input session.
fn own_stream(h: &Shared, client: &Client) -> Option<Arc<Session>> {
    let sessions = h.sessions.lock().unwrap_or_else(PoisonError::into_inner);
    let mut own: Vec<_> = sessions
        .active
        .values()
        .filter(|s| !s.stopping() && s.launch.client.uuid == client.uuid)
        .cloned()
        .collect();
    own.sort_by_key(|s| s.launch.role != Role::Stream);
    own.into_iter().next()
}

/// A guest watches or plays along in another device's game.
fn owns(s: &Session) -> bool {
    s.launch.guest().is_none()
}

fn snapshot(s: Option<&Session>) -> DoctorSnapshot {
    let Some(s) = s else {
        let idle = Facts {
            session_active: false,
            owner: false,
            encoder: None,
            stages: None,
        };
        return doctor::snapshot(idle, &Default::default());
    };
    let mode = s.stream_mode();
    let encoder = EncoderFacts {
        backend: s
            .encoder
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
        codec: butterpollo_media_policy::timing_record::codec_name(s.config.codec).into(),
        hdr: s.hdr_active(),
        width: mode.width,
        height: mode.height,
        fps: mode.fps_millihz.saturating_add(500) / 1000,
        live_bitrate_kbps: s.bitrate.load(Ordering::Acquire),
        fec_percent: u32::try_from(s.fec_percent()).unwrap_or(u32::MAX),
    };
    let window = Duration::from_secs(WINDOW_SECONDS.into());
    let report = s
        .stats
        .stutter_recent(mode.period(), window, Instant::now());
    let rates = s.console_pipeline_rates();
    let facts = Facts {
        session_active: true,
        owner: owns(s),
        encoder: Some(encoder),
        stages: doctor::stages(&report, rates.as_ref()),
    };
    doctor::snapshot(
        facts,
        &s.doctor.lock().unwrap_or_else(PoisonError::into_inner),
    )
}
