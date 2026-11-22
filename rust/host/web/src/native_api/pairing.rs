//! Approve or deny another device's pairing request from a paired device
//! (`docs/native-api.md`, "Approve a device"). The PIN goes down the console's
//! path: the waiting request's `sender`, read by `nvhttp::do_pair` or the
//! WebRTC signalling. Moonlight's handshake derives its key from the PIN and
//! carries no check of it, so the host learns a PIN was wrong only from how the
//! new device's handshake ends, and that attempt is then over: the device starts
//! again with a new PIN. Wrong PINs are counted per address for that reason, as
//! the device chooses its name. Moonlight's `/unpair` after a wrong PIN comes
//! without a certificate, so anyone at the address could send it: it counts for
//! nothing here, and the approval ends when the wait does.
use super::{Failure, Reply, caller, json, permission_required, read_body};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_session::state::{Client, LATER_DEVICE_PERMISSION, PLAY_DEVICE_PERMISSION};
pub use butterpollo_stream::pairing_approvals::{
    Approval, Approvals, Approver, ApproverRevoked, Granted, HANDSHAKE_WAIT, LIST_AND_VIEW,
    NotSaved, Outcome, REQUEST_LIFETIME, approvals, approver_revoked, capped, failed, loud_strike,
    not_saved, paired, paired_by_owner,
};
use butterpollo_stream::{
    state::{Host, PendingPin, Shared},
    tls::Connection,
};
use rubylight_native_api::{
    error::ErrorCode,
    pairing::{PairingApprove, PairingApproved, PairingDeny, PairingRequest, PairingRequests},
};
use std::{net::IpAddr, sync::PoisonError, time::Instant};
use tokio::sync::oneshot;

/// A request that can still be answered.
fn live(request: &PendingPin) -> bool {
    request.created.elapsed() < REQUEST_LIFETIME && !request.sender.is_closed()
}
fn no_request() -> Failure {
    Failure::new(
        ErrorCode::NotFound,
        "This pairing request ended. Start pairing again on the new device.",
    )
}
fn invalid(error: serde_json::Error) -> Failure {
    Failure::new(ErrorCode::InvalidRequest, format!("Invalid body: {error}."))
}

/// Which requests the caller is offered, oldest first.
fn waiting(h: &Host) -> PairingRequests {
    let pins = h.pins.lock().unwrap_or_else(PoisonError::into_inner);
    let mut approvals = approvals(h);
    let mut requests: Vec<_> = pins
        .iter()
        .filter(|(_, p)| live(p) && !approvals.ended(p.address))
        .map(|(id, p)| PairingRequest {
            id: id.clone(),
            name: p.name.clone(),
            address: p.address.to_canonical().to_string(),
            age_seconds: p.created.elapsed().as_secs(),
        })
        .collect();
    requests.sort_by_key(|r| std::cmp::Reverse(r.age_seconds));
    PairingRequests { requests }
}

/// Host control or "Approve new devices". The refusal stays `permission_required`:
/// `host_control_required` would name a wider grant than this route needs.
fn require_approver(client: &Client) -> Result<(), Failure> {
    client
        .may_approve_pairing()
        .then_some(())
        .ok_or_else(|| permission_required("Approve new devices"))
}

/// `GET /pairing/requests`.
pub async fn requests(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    require_approver(&caller(&h, &c)?)?;
    Ok(json(StatusCode::OK, &waiting(&h)))
}

/// Sends the PIN to the waiting request, as the console does, and registers
/// the approval first so the handshake finds it.
fn send_pin(
    h: &Host,
    client: &Client,
    body: &PairingApprove,
) -> Result<(String, IpAddr, oneshot::Receiver<Outcome>), Failure> {
    let mut pins = h.pins.lock().unwrap_or_else(PoisonError::into_inner);
    let request = pins
        .get(&body.id)
        .filter(|p| live(p))
        .ok_or_else(no_request)?;
    let address = request.address.to_canonical();
    let device = request.name.clone();
    if approvals(h).ended(address) {
        return Err(Failure::new(
            ErrorCode::PairingEnded,
            format!(
                "Three wrong PINs from {address}. Pair {device} in the Rubylight console on the PC."
            ),
        ));
    }
    let cooling = approvals(h).cooling(address);
    if let Some(seconds) = cooling {
        return Err(Failure::new(
            ErrorCode::PairingCooldown,
            format!(
                "A wrong PIN just came from {address}. It can be approved again in {seconds} s."
            ),
        ));
    }
    // A Moonlight device gets what a PIN typed in the console gives it by
    // default, under the approver's own bits. A WebRTC device keeps its own
    // default and takes no access choice, so an approver without it may not.
    let permission = if request.certificate.is_empty() {
        if !client.allows(LATER_DEVICE_PERMISSION) {
            return Err(super::permission_required(LIST_AND_VIEW));
        }
        None
    } else {
        // Nothing of Play to hand on would store a device that can't connect.
        let play = PLAY_DEVICE_PERMISSION & client.perm;
        if play == 0 {
            return Err(permission_required(LIST_AND_VIEW));
        }
        Some(play)
    };
    let (outcome, receiver) = oneshot::channel();
    approvals(h).running.insert(
        body.id.clone(),
        Approval {
            approver_name: client.name.clone(),
            device: device.clone(),
            address,
            created: Instant::now(),
            outcome: Some(outcome),
        },
    );
    let request = pins.remove(&body.id).ok_or_else(no_request)?;
    if request
        .sender
        .send((
            body.pin.clone(),
            device.clone(),
            permission,
            Some(client.uuid.clone()),
        ))
        .is_err()
    {
        approvals(h).running.remove(&body.id);
        return Err(no_request());
    }
    Ok((device, address, receiver))
}

/// `POST /pairing/approve`: answers once the new device has finished, or
/// after [`HANDSHAKE_WAIT`].
pub async fn approve(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require_approver(&client)?;
    let body: PairingApprove =
        serde_json::from_slice(&read_body(request).await?).map_err(invalid)?;
    if body.pin.len() != 4 || !body.pin.bytes().all(|c| c.is_ascii_digit()) {
        return Err(Failure::new(ErrorCode::InvalidValue, "The PIN has four digits.").into());
    }
    let (device, address, receiver) = send_pin(&h, &client, &body)?;
    tracing::info!(approver = %client.name, client = %device, %address, "PIN sent for a pairing approved from another device");
    match tokio::time::timeout(HANDSHAKE_WAIT, receiver).await {
        Ok(Ok(Outcome::Paired)) => Ok(json(StatusCode::OK, &PairingApproved { name: device })),
        Ok(Ok(Outcome::WrongPin)) => Err(Failure::new(
            ErrorCode::WrongPin,
            format!("That is not the PIN {device} shows. It has a new one when it asks again."),
        )
        .into()),
        Ok(Ok(Outcome::ApproverRevoked(missing))) => Err(permission_required(missing).into()),
        Ok(Ok(Outcome::NotSaved)) => Err(Failure::new(
            ErrorCode::PairingUnconfirmed,
            format!("The PC could not save {device}."),
        )
        .into()),
        // The handshake went quiet. Moonlight may stop after a wrong PIN without
        // saying so, so it counts as one unless the device still finishes.
        Ok(Err(_)) | Err(_) => {
            let quiet = approvals(&h)
                .running
                .get_mut(&body.id)
                .is_some_and(|approval| approval.outcome.take().is_some());
            if quiet {
                tracing::warn!(approver = %client.name, client = %device, %address, "Pairing approval: the device did not finish");
                loud_strike(&h, address, &device);
            }
            Err(Failure::new(
                ErrorCode::PairingUnconfirmed,
                format!("{device} did not finish pairing. Check the PIN it shows."),
            )
            .into())
        }
    }
}

/// `POST /pairing/deny`: ends the request's wait, as removing it does.
pub async fn deny(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require_approver(&client)?;
    let body: PairingDeny = serde_json::from_slice(&read_body(request).await?).map_err(invalid)?;
    let removed = h
        .pins
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&body.id)
        .ok_or_else(no_request)?;
    tracing::warn!(approver = %client.name, client = %removed.name, address = %removed.address, "Pairing request denied from another device");
    drop(removed);
    Ok(json(StatusCode::OK, &waiting(&h)))
}
