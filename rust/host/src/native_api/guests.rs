//! `GET /session/guests` and its actions (`docs/native-api.md`, "Guests"): the device playing,
//! or one with Host control, answers joins and manages the running game's guests from its own
//! screen. The state is `crate::multiclient`'s.
use super::{Failure, Reply, caller, json, read_body, require_view_or_launch};
use crate::{
    multiclient::{ActionError, View},
    state::Shared,
    tls::Connection,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_core::{
    multiclient::{MAX_LIFETIME, MIN_LIFETIME, Permission},
    native_api::{
        error::ErrorCode,
        guests::{
            GuestApprove, GuestId, GuestInvites, GuestKeyboardMouse, GuestPermission, InviteCreate,
            InviteCreated, InviteRevoke, JoinRequest, SessionGuest, SessionGuests,
        },
    },
    state::{Client, device_uuid},
};
use serde::de::DeserializeOwned;
use std::time::Duration;

/// The list as `client` sees it.
pub(crate) fn project(view: View, client: &Client, manage: bool) -> SessionGuests {
    let device = device_uuid(&client.uuid);
    let you = view
        .guests
        .iter()
        .find(|g| g.device == device)
        .map(|g| g.id);
    SessionGuests {
        policy: view.policy.as_str().into(),
        default_permission: view.default_permission.as_str().into(),
        manage,
        you,
        owner: view.owner_name,
        max_guests: u32::try_from(view.max_guests).unwrap_or(u32::MAX),
        version: view.version,
        keyboard_mouse: view.keyboard_mouse,
        guests: view
            .guests
            .into_iter()
            .map(|g| SessionGuest {
                id: g.id,
                name: g.name,
                permission: g.permission.as_str().into(),
                streaming: g.streaming,
                since: g.since,
                expires: g.expires,
            })
            .collect(),
        waiting: view
            .waiting
            .into_iter()
            .map(|r| JoinRequest {
                id: r.id,
                name: r.name,
                seconds_left: r.seconds_left,
            })
            .collect(),
    }
}

fn answer(h: &Shared, client: &Client) -> Reply {
    let manage = h.device_guests.may_manage(h, client);
    Ok(json(
        StatusCode::OK,
        &project(h.device_guests.view(h), client, manage),
    ))
}

pub async fn list(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    answer(&h, &client)
}

/// The caller, when it may manage guests, and the body.
async fn manager<T: DeserializeOwned>(
    h: &Shared,
    c: &Connection,
    request: Request,
) -> Result<(Client, T), Failure> {
    let client = caller(h, c)?;
    require_view_or_launch(&client)?;
    if !h.device_guests.may_manage(h, &client) {
        return Err(Failure::new(
            ErrorCode::NotSessionOwner,
            "Only the device playing, or one with Host control, manages guests.",
        ));
    }
    let body = serde_json::from_slice(&read_body(request).await?).map_err(|error| {
        Failure::new(ErrorCode::InvalidRequest, format!("Invalid body: {error}."))
    })?;
    Ok((client, body))
}

fn permission(text: &str) -> Result<Permission, Failure> {
    Permission::parse(text).ok_or_else(|| {
        Failure::new(
            ErrorCode::InvalidValue,
            "A guest's permission is view, controller or controller-keyboard-mouse.",
        )
    })
}

fn failed(error: ActionError) -> Failure {
    match error {
        ActionError::NotFound => Failure::new(
            ErrorCode::NotFound,
            "That guest or request is no longer there.",
        ),
        ActionError::Invalid => Failure::new(
            ErrorCode::InvalidValue,
            "The host does not take that for this guest.",
        ),
    }
}

pub async fn approve(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestApprove) = manager(&h, &c, request).await?;
    let permission = match &body.permission {
        Some(text) => permission(text)?,
        None => butterpollo_core::multiclient::default_permission(&h.config_snapshot()),
    };
    let lifetime = match body.seconds {
        Some(seconds) => {
            let lifetime = Duration::from_secs(seconds);
            if !(MIN_LIFETIME..=MAX_LIFETIME).contains(&lifetime) {
                return Err(Failure::new(
                    ErrorCode::InvalidValue,
                    "A guest's time is 5 minutes to 7 days, or until the game ends.",
                ));
            }
            Some(lifetime)
        }
        None => None,
    };
    h.device_guests
        .approve(body.id, permission, lifetime)
        .map_err(failed)?;
    tracing::info!(by = %client.name, request = body.id, "join approved from a device");
    answer(&h, &client)
}

pub async fn deny(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestId) = manager(&h, &c, request).await?;
    h.device_guests.deny(body.id).map_err(failed)?;
    answer(&h, &client)
}

pub async fn kick(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestId) = manager(&h, &c, request).await?;
    h.device_guests.kick(&h, body.id).map_err(failed)?;
    answer(&h, &client)
}

pub async fn revoke(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestId) = manager(&h, &c, request).await?;
    h.device_guests.revoke(&h, body.id).map_err(failed)?;
    answer(&h, &client)
}

pub async fn set_permission(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestPermission) = manager(&h, &c, request).await?;
    let permission = permission(&body.permission)?;
    h.device_guests
        .set_permission(&h, body.id, permission)
        .map_err(failed)?;
    answer(&h, &client)
}

pub async fn keyboard_mouse(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestKeyboardMouse) = manager(&h, &c, request).await?;
    h.device_guests
        .give_keyboard_mouse(body.id)
        .map_err(failed)?;
    answer(&h, &client)
}

/// The caller, when it may invite: Approve new devices or Host control.
fn inviter(h: &Shared, c: &Connection) -> Result<Client, Failure> {
    let client = caller(h, c)?;
    if !client.may_approve_pairing() {
        return Err(Failure::new(
            ErrorCode::PermissionRequired,
            "Inviting someone takes Approve new devices or Host control.",
        ));
    }
    Ok(client)
}

fn invites_view(h: &Shared) -> Result<GuestInvites, Failure> {
    serde_json::from_value(crate::invites::view(h))
        .map_err(|error| Failure::new(ErrorCode::Internal, format!("{error}")))
}

/// `GET /session/guests/invites`: the links waiting and the devices paired from one.
pub async fn invites(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    inviter(&h, &c)?;
    let view = tokio::task::spawn_blocking(move || invites_view(&h))
        .await
        .map_err(|_| Failure::new(ErrorCode::Internal, "The invite list failed."))??;
    Ok(json(StatusCode::OK, &view))
}

/// `POST /session/guests/invites`: a new link, shown once.
pub async fn invite(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = inviter(&h, &c)?;
    let body: InviteCreate =
        serde_json::from_slice(&read_body(request).await?).map_err(|error| {
            Failure::new(ErrorCode::InvalidRequest, format!("Invalid body: {error}."))
        })?;
    let wanted = body.permission.as_deref().map(permission).transpose()?;
    let lifetime = match body.seconds {
        Some(seconds) => {
            let lifetime = Duration::from_secs(seconds);
            if !(MIN_LIFETIME..=MAX_LIFETIME).contains(&lifetime) {
                return Err(Failure::new(
                    ErrorCode::InvalidValue,
                    "An invite works from 300 to 604800 seconds.",
                ));
            }
            Some(lifetime)
        }
        None => None,
    };
    let created = tokio::task::spawn_blocking(move || {
        crate::invites::create(&h, wanted, lifetime, Some(&client))
    })
    .await
    .map_err(|_| Failure::new(ErrorCode::Internal, "The invite failed."))?
    .map_err(|failure| {
        let code = match failure {
            crate::invites::CreateFailure::NotAllowed => ErrorCode::PermissionRequired,
            crate::invites::CreateFailure::PairingOff => ErrorCode::TemporarilyUnavailable,
            crate::invites::CreateFailure::Policy(_) => ErrorCode::InvalidValue,
            crate::invites::CreateFailure::Internal(_) => ErrorCode::Internal,
        };
        Failure::new(code, failure.message())
    })?;
    Ok(json(
        StatusCode::OK,
        &InviteCreated {
            id: created.id,
            link: created.link,
            expires: created.expires,
            permission: created.permission.as_str().into(),
        },
    ))
}

/// `POST /session/guests/invites/revoke`: takes back a link, or removes a device paired from one.
pub async fn revoke_invite(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    inviter(&h, &c)?;
    let body: InviteRevoke =
        serde_json::from_slice(&read_body(request).await?).map_err(|error| {
            Failure::new(ErrorCode::InvalidRequest, format!("Invalid body: {error}."))
        })?;
    let view = tokio::task::spawn_blocking(move || {
        let found = match (body.id, body.uuid.as_deref()) {
            (Some(id), None) => Ok(crate::invites::revoke(&h, id)),
            (None, Some(uuid)) => crate::invites::remove(&h, uuid)
                .map_err(|error| Failure::new(ErrorCode::Internal, format!("{error:#}"))),
            _ => Err(Failure::new(
                ErrorCode::InvalidRequest,
                "Send the id of a link or the uuid of an invited device.",
            )),
        }?;
        if !found {
            return Err(Failure::new(
                ErrorCode::NotFound,
                "That invite or invited device is no longer there.",
            ));
        }
        invites_view(&h)
    })
    .await
    .map_err(|_| Failure::new(ErrorCode::Internal, "Taking the invite back failed."))??;
    Ok(json(StatusCode::OK, &view))
}
