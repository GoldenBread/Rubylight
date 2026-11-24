//! A Linux desktop stream's virtual display: the same request as on Windows (the device, its
//! settings, the app and the host), sized by the same rules, made by the Linux display crate
//! as a monitor of the device's mode on the virtual display pool, and captured by name
//! through KWin (`kwin-output:<connector>`). The display's drop restores the monitors.
use super::policy::{
    client_display, virtual_display_identity, virtual_display_layout, virtual_display_mode,
    virtual_display_request,
};
use crate::state::{Launch, Shared};
use anyhow::Result;
use butterpollo_config::config::Config;
use butterpollo_media_policy::display_policy::Arrangement;
use butterpollo_media_policy::rtsp::Negotiated;
use butterpollo_session::session::Role;
use butterpollo_win_display::display;

/// The virtual display a stream shows, and the capture target that shows it.
pub(super) struct Private {
    pub(super) target: String,
    pub(super) display: display::VirtualDisplay,
}

/// The stream's virtual display, when one is asked for and the PC can make one. None when
/// none is asked for, or when none can be made (the launch's card says why, and the stream
/// shows the desktop's own monitor as on Windows).
pub(super) fn prepare(
    h: &Shared,
    launch: &Launch,
    stream: &Negotiated,
    config: &Config,
) -> Result<Option<Private>> {
    if launch.role != Role::Stream {
        return Ok(None);
    }
    let app = crate::stream::launch_app(h, launch);
    let output_override = app
        .as_ref()
        .and_then(|a| a.extra.get("display-output"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            launch
                .client
                .extra
                .get("output_name_override")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
        });
    let mode = virtual_display_mode(
        config,
        &launch.client,
        app.as_ref(),
        display::virtual_display_os(),
    );
    let request = virtual_display_request(launch, app.as_ref(), config, mode, output_override);
    if !request.requested() {
        launch.warnings.clear("display_virtual");
        return Ok(None);
    }
    if !request.uses_virtual(false, display::ensure_virtual_display_available) {
        let status = display::virtual_display_status();
        let reason = status["reason"].as_str().unwrap_or("it is not ready");
        launch.warnings.set(
            "display_virtual",
            format!("Streaming the desktop's own monitor because the virtual display is unavailable: {reason}."),
        );
        return Ok(None);
    }
    launch.warnings.clear("display_virtual");
    let layout =
        virtual_display_layout(config, &launch.client, app.as_ref(), client_display(launch));
    let arrangement = Arrangement::parse(layout).unwrap_or_else(|_| {
        tracing::warn!(layout, "unknown virtual display layout; using exclusive");
        Arrangement::Exclusive
    });
    let app_scale = app
        .as_ref()
        .and_then(|a| a.extra.get("scale-factor"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(100);
    let client_scale = launch
        .options
        .get("scaleFactor")
        .and_then(|value| value.parse().ok())
        .unwrap_or(100);
    let (display_width, display_height) = stream.display_size();
    let (render_width, render_height) = butterpollo_media_policy::display_policy::render_dimensions(
        display_width,
        display_height,
        client_scale,
        app_scale,
    );
    let rate = butterpollo_media_policy::framegen::Rate(stream.fps_millihz());
    let wanted = butterpollo_media_policy::display_policy::display_request_rate(
        config,
        render_width,
        render_height,
        rate,
        false,
        true,
    )?;
    let (width, height) = super::scale::virtual_display_size(
        config,
        wanted.resolution.unwrap_or((render_width, render_height)),
        (display_width, display_height),
    );
    let stable_id = if mode == "shared" {
        "shared".to_owned()
    } else {
        virtual_display_identity(config, launch, app.as_ref())
    };
    let scale_percent = u32::try_from(config.integer("dd_virtual_display_scale", 0))
        .ok()
        .filter(|percent| *percent > 0);
    let display = display::VirtualDisplay::create_with(&display::PrivateRequest {
        stable_id,
        width,
        height,
        refresh_millihz: wanted.refresh.unwrap_or(rate.0),
        arrangement,
        scale_percent,
    })?;
    Ok(Some(Private {
        target: format!("kwin-output:{}", display.name),
        display,
    }))
}
