//! Console USB forwarding API parity test extracted from `web_parity_tests`.
use super::{Fixture, Instant, StatusCode, Value, json, request, value};

#[tokio::test]
async fn the_console_lists_forwarded_usb_devices_and_detaches_them() {
    use rubylight_protocol::usb::{UsbMessage, UsbReason, UsbState};
    let f = Fixture::new();
    f.host
        .usb
        .set_vhci(butterpollo_stream::usb_forward::test_support::installed());
    let client = butterpollo_stream::usb_forward::test_support::client(true);
    f.host.usb.on_message(
        "usb-session",
        &client,
        true,
        4,
        UsbMessage::AttachRequest {
            request_id: 3,
            vid: 0x045e,
            pid: 0x028e,
            class: 0xff,
            speed: 2,
            busid: "2-1".into(),
            name: "Gamepad".into(),
        },
    );
    drop(f.host.usb.take("usb-session", Instant::now()));
    let listed = value(
        request(&f, "GET", "/api/usb", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let device = &listed["devices"][0];
    assert_eq!(
        (
            &device["client"],
            &device["name"],
            &device["vid"],
            &device["busid"]
        ),
        (
            &json!("phone"),
            &json!("Gamepad"),
            &json!(0x045e),
            &json!("2-1")
        )
    );
    assert_eq!(device["state"], "waiting");
    let id = device["id"].clone();
    value(
        request(&f, "POST", "/api/usb/detach", json!({"id": 999})).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    value(
        request(&f, "POST", "/api/usb/detach", json!({"id": id})).await,
        StatusCode::OK,
    )
    .await;
    assert!(matches!(
        f.host.usb.take("usb-session", Instant::now())[..],
        [
            UsbMessage::Detach { request_id: 3 },
            UsbMessage::Status {
                request_id: 3,
                state: UsbState::Detached,
                reason: UsbReason::None
            }
        ]
    ));
    let listed = value(
        request(&f, "GET", "/api/usb", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed["devices"], json!([]));
}
