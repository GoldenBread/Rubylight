//! `GET /session/curtain` and `POST /session/curtain/dismiss` (`docs/native-api.md`, "Launch
//! curtain"): where the start of a game is, for a device that keeps a curtain over its stream,
//! and lifting it. Reading needs View or Launch; lifting needs Launch and the launch's
//! ownership (`butterpollo_stream::launch_curtain`). The same state travels on the control
//! stream as `0x5548`.
use super::{Failure, Reply, caller, json, read_body, require, require_view_or_launch};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_session::native::LAUNCH;
use butterpollo_stream::{launch_curtain::DismissRefused, state::Shared, tls::Connection};
use rubylight_native_api::{curtain::CurtainDismissRequest, error::ErrorCode};

/// `GET /session/curtain`.
pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    Ok(json(StatusCode::OK, &h.curtain.snapshot(&client.uuid)))
}

/// `POST /session/curtain/dismiss` with `{}`.
pub async fn dismiss(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require(&client, LAUNCH, "Launch applications")?;
    let bytes = read_body(request).await?;
    if !bytes.iter().all(u8::is_ascii_whitespace) {
        serde_json::from_slice::<CurtainDismissRequest>(&bytes).map_err(|error| {
            Failure::new(
                ErrorCode::InvalidRequest,
                format!("The body is not a curtain request: {error}."),
            )
        })?;
    }
    match h.curtain.dismiss(&client.uuid) {
        Ok(snapshot) => Ok(json(StatusCode::OK, &snapshot)),
        Err(DismissRefused::NoCurtain) => {
            Err(Failure::new(ErrorCode::NoSession, "No game is starting with a curtain.").into())
        }
        Err(DismissRefused::NotOwner) => Err(Failure::new(
            ErrorCode::NotSessionOwner,
            "Another device started this game; only it can show the PC's screen.",
        )
        .into()),
    }
}
