//! The stream card's guests from paired devices (docs/multiclient.md): `GET /api/session-guests`
//! lists the devices that joined the running game and the joins waiting, and the POSTs below
//! answer joins and manage guests as the device playing can. Console-only: API tokens cannot
//! reach them (they are not in the token catalogue).
use super::error;
use crate::{multiclient::ActionError, state::Shared};
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_session::multiclient::{
    MAX_LIFETIME, MIN_LIFETIME, Permission, default_permission,
};
use serde_json::{Value, json};
use std::time::Duration;

fn id(data: &Value) -> Option<u32> {
    data.get("id")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
}

fn permission(data: &Value) -> Option<Permission> {
    data.get("permission")
        .and_then(Value::as_str)
        .and_then(Permission::parse)
}

fn answer(h: &Shared, result: Result<(), ActionError>) -> Response {
    match result {
        Ok(()) => Json(json!(h.device_guests.view(h))).into_response(),
        Err(ActionError::NotFound) => error(
            StatusCode::NOT_FOUND,
            "that guest or request is no longer there",
        ),
        Err(ActionError::Invalid) => error(
            StatusCode::BAD_REQUEST,
            "the host does not take that for this guest",
        ),
    }
}

pub(super) fn handle(h: &Shared, method: &Method, path: &str, data: &Value) -> Response {
    let guests = &h.device_guests;
    let missing_id = || error(StatusCode::BAD_REQUEST, "id must be a guest or request id");
    let bad_permission = || {
        error(
            StatusCode::BAD_REQUEST,
            "permission must be view, controller or controller-keyboard-mouse",
        )
    };
    match (method.as_str(), path) {
        ("GET", "/api/session-guests") => Json(json!(guests.view(h))).into_response(),
        ("POST", "/api/session-guests/approve") => {
            let Some(id) = id(data) else {
                return missing_id();
            };
            let permission = match data.get("permission") {
                None | Some(Value::Null) => default_permission(&h.config_snapshot()),
                Some(_) => match permission(data) {
                    Some(permission) => permission,
                    None => return bad_permission(),
                },
            };
            let lifetime = match data.get("seconds") {
                None | Some(Value::Null) => None,
                Some(seconds) => match seconds.as_u64().map(Duration::from_secs) {
                    Some(lifetime) if (MIN_LIFETIME..=MAX_LIFETIME).contains(&lifetime) => {
                        Some(lifetime)
                    }
                    _ => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            "seconds must be 300 to 604800, or null until the game ends",
                        );
                    }
                },
            };
            tracing::info!(request = id, "join approved in the console");
            answer(h, guests.approve(id, permission, lifetime))
        }
        ("POST", "/api/session-guests/deny") => match id(data) {
            Some(id) => answer(h, guests.deny(id)),
            None => missing_id(),
        },
        ("POST", "/api/session-guests/kick") => match id(data) {
            Some(id) => answer(h, guests.kick(h, id)),
            None => missing_id(),
        },
        ("POST", "/api/session-guests/revoke") => match id(data) {
            Some(id) => answer(h, guests.revoke(h, id)),
            None => missing_id(),
        },
        ("POST", "/api/session-guests/permission") => match (id(data), permission(data)) {
            (Some(id), Some(permission)) => answer(h, guests.set_permission(h, id, permission)),
            (None, _) => missing_id(),
            (_, None) => bad_permission(),
        },
        ("POST", "/api/session-guests/keyboard-mouse") => match data.get("id") {
            None | Some(Value::Null) => answer(h, guests.give_keyboard_mouse(None)),
            Some(_) => match id(data) {
                Some(id) => answer(h, guests.give_keyboard_mouse(Some(id))),
                None => missing_id(),
            },
        },
        ("GET", "/api/session-guests/invites") => Json(crate::invites::view(h)).into_response(),
        ("POST", "/api/session-guests/invites") => {
            let wanted = match data.get("permission") {
                None | Some(Value::Null) => None,
                Some(_) => match permission(data) {
                    Some(permission) => Some(permission),
                    None => return bad_permission(),
                },
            };
            let lifetime = match data.get("seconds") {
                None | Some(Value::Null) => None,
                Some(seconds) => match seconds.as_u64().map(Duration::from_secs) {
                    Some(lifetime) if (MIN_LIFETIME..=MAX_LIFETIME).contains(&lifetime) => {
                        Some(lifetime)
                    }
                    _ => {
                        return error(StatusCode::BAD_REQUEST, "seconds must be 300 to 604800");
                    }
                },
            };
            match crate::invites::create(h, wanted, lifetime, None) {
                Ok(created) => Json(json!({
                    "id": created.id,
                    "link": created.link,
                    "expires": created.expires,
                    "permission": created.permission.as_str(),
                }))
                .into_response(),
                Err(failure) => error(StatusCode::BAD_REQUEST, &failure.message()),
            }
        }
        ("POST", "/api/session-guests/invites/revoke") => {
            let found = match (id(data), data.get("uuid").and_then(Value::as_str)) {
                (Some(id), None) => Ok(crate::invites::revoke(h, id)),
                (None, Some(uuid)) => crate::invites::remove(h, uuid),
                _ => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "send the id of a link or the uuid of an invited device",
                    );
                }
            };
            match found {
                Ok(true) => Json(crate::invites::view(h)).into_response(),
                Ok(false) => error(
                    StatusCode::NOT_FOUND,
                    "that invite or invited device is no longer there",
                ),
                Err(failure) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}")),
            }
        }
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}
