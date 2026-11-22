//! `GET /wake`: the PC's wake targets and its sleep state (`docs/native-api.md`, "Wake
//! targets"). It only reads; NIC settings change only with consent, from the installer or the
//! admin console.
use super::{Failure, Reply, caller, json, require};
use axum::{Extension, extract::State, http::StatusCode};
use butterpollo_session::native::{LAUNCH, LIST, VIEW};
use butterpollo_stream::{state::Shared, tls::Connection, wake};

pub async fn read(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require(&client, LIST, "List applications")
        .or_else(|_| require(&client, VIEW, "View streams"))
        .or_else(|_| require(&client, LAUNCH, "Launch applications"))?;
    let local = c.local.ip();
    let wake = tokio::task::spawn_blocking(move || wake::readiness(&h, false, Some(local)))
        .await
        .map_err(|error| Failure::internal("read the wake targets", error))?
        .map_err(|error| Failure::internal("read the wake targets", format!("{error:#}")))?;
    Ok(json(StatusCode::OK, &wake))
}
