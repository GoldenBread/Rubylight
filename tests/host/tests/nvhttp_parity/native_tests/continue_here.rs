//! `POST /session/continue-here` and what the session says around it.
use super::*;
use butterpollo_session::session::{Seat, Transport};
use butterpollo_stream::{
    nvhttp::{WebRtcLaunch, WebRtcLaunchError, launch_webrtc, terminate_refused},
    state::Launch,
};

fn game_owner(s: &Setup) -> String {
    let current = s.host().current_app.lock().unwrap();
    current.as_ref().unwrap().owner.clone()
}
async fn press(s: &Setup, connection: &Connection) -> Reply {
    api(&s.f, connection, "POST", "/session/continue-here").await
}
async fn session_of(s: &Setup, connection: &Connection) -> ApiSession {
    api(&s.f, connection, "GET", "/session").await.session()
}
/// A session with this launch, running.
fn running(s: &Setup, launch: Launch) -> Arc<Session> {
    let session = Session::new(launch, Default::default());
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    session
}

#[path = "continue_here/press_and_guards_tests.rs"]
mod press_and_guards_tests;
#[path = "continue_here/recovery_and_expiry_tests.rs"]
mod recovery_and_expiry_tests;
