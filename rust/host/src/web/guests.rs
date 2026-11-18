//! The Stream page's guests (docs/browser-guests.md). `GET /api/guests` lists the invite links
//! and the guests watching, never a token, and the guest page's port. `POST /api/guests/invite`
//! makes a link lasting `seconds` with a `permission` for the `address` the owner puts in it,
//! and answers with its token, the only time the host has it; with `internet` the guest ports
//! stay mapped on the router while it works. `GET`/`POST /api/guests/internet` is what the
//! router did with those ports and the address such a link should carry. `GET /api/guests/addresses` is
//! this PC's LAN addresses, for a console opened on this PC. `POST /api/guests/revoke` withdraws
//! a link and disconnects whoever joined with it; `POST /api/guests/kick` disconnects one guest,
//! whose link keeps working. Console-only: API tokens cannot reach them, and the guest port
//! serves nothing of the console's.

use super::error;
use crate::{state::Shared, webrtc::guests::InviteInfo};
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_core::webrtc::guests::{InviteError, Permission};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) fn handle(h: &Shared, method: &Method, path: &str, data: &Value) -> Response {
    match (method.as_str(), path) {
        ("GET", "/api/guests") => {
            let (port, media) = {
                let config = h.config.read().unwrap();
                let media = config.webrtc_media_port();
                let media: Vec<u16> = (1..=crate::webrtc::guests::MAX_GUESTS)
                    .filter_map(|seat| media.checked_add(seat))
                    .collect();
                (config.ports().map(|ports| ports.guest), media)
            };
            let mut status = json!(h.guests.status());
            status["port"] = json!(port.ok());
            status["media_ports"] = json!(media);
            Json(status).into_response()
        }
        ("GET", "/api/guests/addresses") => Json(json!({"addresses": addresses()})).into_response(),
        // The invite dialog's "On the internet": what the router did with the guest ports, and
        // the address a link should carry. POST asks for the ports first (for a while, before
        // a link is made), GET reads the answer.
        ("GET" | "POST", "/api/guests/internet") => {
            if method == Method::POST {
                h.guests.prepare_internet();
            }
            match crate::internet_play::guest_reach(h) {
                Ok(reach) => Json(reach).into_response(),
                Err(cause) => error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("The guest port is unknown: {cause:#}"),
                ),
            }
        }
        ("POST", "/api/guests/invite") => {
            let Some(permission) = data
                .get("permission")
                .and_then(Value::as_str)
                .and_then(Permission::parse)
            else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "permission must be view, controller or controller-keyboard-mouse",
                );
            };
            let Some(seconds) = data.get("seconds").and_then(Value::as_u64) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "seconds must be how long the link lasts",
                );
            };
            let address = match data.get("address") {
                None | Some(Value::Null) => None,
                Some(address) => match address.as_str().filter(|a| valid_address(a)) {
                    Some(address) => Some(address.to_owned()),
                    None => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            "address must be a host name or IP address, with an optional port",
                        );
                    }
                },
            };
            let internet = match data.get("internet") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(internet)) => *internet,
                Some(_) => {
                    return error(StatusCode::BAD_REQUEST, "internet must be true or false");
                }
            };
            match h
                .guests
                .create_for(permission, Duration::from_secs(seconds), address, internet)
            {
                Ok((invite, token)) => {
                    tracing::info!(
                        invite = %invite.id,
                        permission = permission.as_str(),
                        expires = invite.expires,
                        internet,
                        "guest invite created"
                    );
                    let info = InviteInfo {
                        internet,
                        ..InviteInfo::from(&invite)
                    };
                    Json(json!({
                        "invite": info,
                        "token": token,
                    }))
                    .into_response()
                }
                Err(refusal @ InviteError::Lifetime) => {
                    error(StatusCode::BAD_REQUEST, &refusal.to_string())
                }
                Err(refusal @ InviteError::Full) => {
                    error(StatusCode::CONFLICT, &refusal.to_string())
                }
            }
        }
        ("POST", "/api/guests/revoke") => {
            let Some(id) = data.get("id").and_then(Value::as_str) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "id must be the invite the panel showed",
                );
            };
            if !h.guests.revoke(id) {
                return error(StatusCode::NOT_FOUND, "There is no such invite link.");
            }
            tracing::info!(invite = %id, "guest invite revoked");
            Json(json!({})).into_response()
        }
        ("POST", "/api/guests/kick") => {
            let Some(id) = data.get("id").and_then(Value::as_u64) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "id must be the guest the panel showed",
                );
            };
            if !h.guests.kick(id) {
                return error(StatusCode::NOT_FOUND, "That guest is not watching.");
            }
            tracing::info!(guest = id, "guest kicked");
            Json(json!({})).into_response()
        }
        _ => error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    }
}

/// What a link's address may be: a host name, an IPv4 address or a bracketed IPv6 one, with an
/// optional port. The host looks it up when a guest joins with the link.
fn valid_address(address: &str) -> bool {
    let (host, port) = match address.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((v6, rest)) if v6.parse::<std::net::Ipv6Addr>().is_ok() => (v6, rest),
            _ => return false,
        },
        None => match address.split_once(':') {
            Some((host, _)) => (host, &address[host.len()..]),
            None => (address, ""),
        },
    };
    let port_ok = port.is_empty()
        || port
            .strip_prefix(':')
            .and_then(|p| p.parse::<u16>().ok())
            .is_some_and(|p| p != 0);
    let host_ok = address.starts_with('[')
        || (!host.is_empty()
            && host.len() <= 253
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'));
    port_ok && host_ok
}

/// This PC's LAN addresses, for a link made on a console opened at `localhost`, which no guest
/// can open. Empty, and logged, when Windows does not list them.
fn addresses() -> Vec<String> {
    butterpollo_windows::net::lan_addresses().unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the LAN addresses for a guest link could not be read");
        Vec::new()
    })
}
