//! Per-frame encoder preparation (luminance, overlay, HDR metadata, invalidation, client
//! cursor, focus), submission, and encode-failure handling for the video session loop.

use super::super::*;
use super::inline_send::InlineSender;
use super::rebuild::{apply_frame_cap, rollback_mode_switch};
use super::timings::{TimingsWindow, emit_claim_trace};

/// Result of submitting one captured picture to the active encoder.
pub(super) enum EncodeStep {
    Encoded {
        output: Vec<butterpollo_win_graphics::encoder::Encoded>,
        begin: Instant,
        fresh: bool,
        recovering: bool,
        presented_captured: Instant,
        last_submitted: Option<Instant>,
    },
    /// Recoverable error handled (device loss, mode rollback, or rebuild scheduled).
    Continue,
}

/// Configures per-frame encoder state and encodes `image`.
#[allow(clippy::too_many_arguments)]
pub(super) fn encode_frame(
    thread: &SessionThread<'_>,
    c: &Config,
    runtime_config: &Config,
    prepared: &Arc<crate::display_session::Ready>,
    progress: &crate::stall_watch::Progress,
    latest: &Source,
    active: &mut Encoder,
    mut readback: impl FnMut(&GpuImage) -> Result<butterpollo_win_graphics::capture::Image>,
    stream: &mut butterpollo_media_policy::rtsp::Negotiated,
    resolution_cap: Option<(u32, u32)>,
    switching: &mut Option<(butterpollo_media_policy::reconfigure::Mode, Instant)>,
    rebuild_encoder: &mut bool,
    applied_luminance: &mut Option<(f32, f32)>,
    metadata_due: &mut Instant,
    confirm_keyframe: &mut Option<Instant>,
    menu: &mut crate::couch_menu::VideoOverlay,
    enhancer: &crate::nvidia_enhance::Enhancer,
    focus: &mut crate::stream_focus::Focus,
    focus_verify: Option<u32>,
    pointer_left_out: &mut bool,
    pointer_warned: &mut bool,
    backend: &str,
    inline_sender: &InlineSender,
    recovery: &mut EncoderRecovery,
    counted_failures: &mut CountedFailures,
    recovery_packetizer: &mut VideoPacketizer,
    fec_whole: &mut ((usize, usize, bool), usize),
    fec_whole_due: &mut Instant,
    checks: &mut crate::video_send::link_watch::CheckTimes,
    mut frame_checks: Duration,
    timings: &mut TimingsWindow,
    admission_trace: &mut Option<AdmissionTrace>,
    trace_attempt: &mut u64,
    trace_send: bool,
    start: Instant,
    first_seen: Option<(usize, Instant, Option<Duration>, Option<Instant>)>,
    raw_image: &Arc<GpuImage>,
    image: &Arc<GpuImage>,
    last_image: &Option<std::sync::Weak<GpuImage>>,
    pacing: &capture::PacingSnapshot,
    admission_recovering: bool,
    encoding: usize,
    sending: usize,
    encoding_limit: usize,
    rebuilt: bool,
    peer: SocketAddr,
    period: Duration,
    direct_encoder_input: bool,
) -> Result<EncodeStep> {
    let SessionThread { m, s, .. } = *thread;
    // RTX HDR uses its own SDR white and scale; otherwise follow Windows' SDR brightness for HDR
    // and an HDR desktop (FP16, or PQ that an SDR stream tone-maps).
    let wanted_luminance = enhancer.luminance().or_else(|| {
        (stream.hdr || image.pixel != butterpollo_win_graphics::capture::Pixel::Bgra8)
            .then(|| (image.gpu.sdr_white_nits(), 1.0))
    });
    if let Some((white, scale)) = wanted_luminance
        && (rebuilt || *applied_luminance != Some((white, scale)))
    {
        active.set_luminance(white, scale);
        *applied_luminance = Some((white, scale));
    }
    if menu.dirty || (rebuilt && menu.has_overlay()) {
        active.set_overlay(menu.for_encoder([stream.width, stream.height]));
    }
    // Frames of a stream that left slice delivery return whole.
    inline_sender.delivery.sync(&s.stats);
    if inline_sender.delivery.joins() {
        active.join_slices();
    }
    if rebuilt {
        active.set_next_frame(match inline_sender.video_sender.get() {
            Some(sender) => sender.next_wire_frame()?,
            None => inline_sender.next_wire_frame.get(),
        });
    }
    let begin = Instant::now();
    let fresh = last_image
        .as_ref()
        .is_none_or(|previous| !std::ptr::eq(previous.as_ptr(), Arc::as_ptr(raw_image)));
    if let Some(admission) = admission_trace.as_mut() {
        emit_claim_trace(
            admission,
            trace_attempt,
            start,
            first_seen,
            raw_image,
            image,
            begin,
            pacing.picture_interval,
            latest,
            recovery,
            s,
            fresh,
            admission_recovering,
            encoding,
            sending,
            encoding_limit,
            rebuilt,
        );
    }
    timings.record_claim_age(fresh, raw_image.repeat, image, begin);
    if Instant::now() >= *metadata_due {
        let checks_started = Instant::now();
        let mut display_metadata = image.gpu.hdr_metadata();
        if let Some(peak) = enhancer.hdr_peak(runtime_config) {
            // RTX HDR generated this picture at its own peak; the
            // SDR monitor's peak does not describe it.
            display_metadata = display_metadata.generated(peak);
        }
        frame_checks += checks_started.elapsed();
        let metadata = m.set_display_hdr_metadata(s, display_metadata);
        active.set_hdr_metadata(metadata);
        // A receiver's report (0x5531) arrives after the stream starts.
        active.set_hdr_target(s.hdr_target_nits());
        *metadata_due = Instant::now() + Duration::from_secs(1);
    }
    let invalidation = s.invalidation.lock().unwrap().take();
    if let Some((first, last)) = invalidation {
        if !active.invalidate_ref_frames(first, last) {
            s.request_recovery_keyframe();
        } else if active.needs_recovery_confirmation() {
            // A decoder that cannot follow the long-term
            // reference stays frozen, and Moonlight does not
            // ask again: a keyframe a second later bounds that
            // unless post-RFI feedback proves clean delivery.
            s.begin_rfi_confirmation();
            confirm_keyframe.get_or_insert(Instant::now() + RFI_CONFIRM_KEYFRAME);
        }
    }
    poll_rfi_confirmation(s, confirm_keyframe, Instant::now());
    let idr = s.idr.swap(false, Ordering::AcqRel);
    if idr {
        *confirm_keyframe = None;
        s.clear_rfi_confirmation();
    }
    let recovering = idr || invalidation.is_some();
    // Stream DVR's keyframe is the host's own: no recovery, nothing skipped or re-sent.
    // Taken either way: a recovery keyframe also serves Stream DVR.
    let dvr_keyframe = s.dvr.take_keyframe();
    let keyframe = idr || dvr_keyframe;
    // A client-chosen FEC share moves the encoder's part of the
    // budget at a fence (an IDR or a new encoder) or after a
    // statistical FEC report; a new rate needs no keyframe.
    let bitrate = s.encoder_bitrate(idr || rebuilt);
    apply_frame_cap(
        s,
        c,
        active,
        peer,
        period,
        bitrate,
        &inline_sender.link_watch,
        recovery_packetizer,
        fec_whole,
        fec_whole_due,
        &mut frame_checks,
    );
    checks.record(frame_checks);
    // The device draws the pointer: only Desktop Duplication supplies
    // it apart, so with any other capture it stays in the video and the
    // device is told to hide its own.
    let leave_pointer_out = s.client_cursor.active()
        && {
            let ddx = backend == "ddx";
            let pointer = image.pointer().filter(|_| ddx).map(|p| {
                butterpollo_input_policy::client_cursor::Pointer {
                    width: p.width,
                    height: p.height,
                    hotspot: p.hotspot,
                    logic: p.logic,
                    pixels: p.pixels,
                }
            });
            if s.client_cursor
                .note(pointer, &image.gpu.display.display_name)
            {
                let _ = m.control_wake.send(&[1]);
            }
            if !ddx && !std::mem::replace(pointer_warned, true) {
                tracing::warn!(
                    capture = backend,
                    "the device-drawn cursor needs Desktop Duplication capture; the pointer stays in the video"
                );
            }
            ddx
        };
    let pointer_toggled = leave_pointer_out != *pointer_left_out;
    *pointer_left_out = leave_pointer_out;
    // A repeat with a new panel, or with the pointer put in or left out,
    // is a new picture.
    active.set_repeat((!fresh || raw_image.repeat) && !menu.dirty && !pointer_toggled);
    menu.dirty = false;
    // A map that failed on a frame turned focus off in
    // the encoder; keep it off for the stream, loudly.
    if let Some(error) = active.take_focus_failure() {
        focus.failed(&error, &s.encoder_focus_state, &s.launch.warnings);
    }
    focus.update(&s.encoder_focus, &s.encoder_focus_state, &s.launch.warnings);
    let wanted = focus.settings(|| {
        butterpollo_win_graphics::encoder::focus_cursor(image, (stream.width, stream.height))
    });
    if let Err(error) = focus.apply(wanted, |settings| {
        active.set_focus_verify(focus_verify);
        active.set_focus(settings)
    }) {
        focus.failed(&error, &s.encoder_focus_state, &s.launch.warnings);
        active.set_focus(None)?;
    }
    let mut modified_image = None;
    let presented_image = if !fresh || leave_pointer_out {
        let copy = modified_image.insert(image.as_ref().clone());
        if !fresh {
            copy.captured = Instant::now();
        }
        if leave_pointer_out {
            copy.leave_pointer_out();
        }
        &*copy
    } else {
        image.as_ref()
    };
    progress.mark(crate::stall_watch::Phase::Encode);
    let encode_trace_started = trace_send.then(Instant::now);
    let encoded = (|| -> Result<Vec<butterpollo_win_graphics::encoder::Encoded>> {
        #[cfg(debug_assertions)]
        crate::soak_fault::check("encoder failure")?;
        #[cfg(debug_assertions)]
        crate::soak_fault::check("DXGI_ERROR_DEVICE_REMOVED")?;
        Ok(if direct_encoder_input {
            active.encode_gpu(presented_image, keyframe, bitrate)?
        } else {
            active.encode(&readback(presented_image)?, keyframe, bitrate)?
        })
    })();
    if let Some(at) = encode_trace_started {
        tracing::trace!(
            target: "pacing",
            stream_id = %s.launch.id,
            attempt = *trace_attempt,
            host_acquired = ?image.acquired,
            submitted_presentation = ?presented_image.captured,
            encode_call_us = at.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
            submitted = active
                .last_submitted()
                .filter(|submitted| *submitted >= at)
                .map_or(0, |submitted| submitted.saturating_duration_since(start).as_micros() as u64),
            outputs = encoded.as_ref().map_or(0, Vec::len),
            ok = encoded.is_ok(),
            idr,
            fresh,
            "encode returned"
        );
    }
    match encoded {
        Ok(output) => Ok(EncodeStep::Encoded {
            output: recovery.output(output, Instant::now)?,
            begin,
            fresh,
            recovering,
            presented_captured: presented_image.captured,
            last_submitted: active.last_submitted(),
        }),
        Err(error) => {
            if let Some(loss) = active
                .device_removed()
                .or_else(|| DeviceLost::d3d11(&image.gpu.device))
                .or_else(|| DeviceLost::from_error(&error))
            {
                recovery.device_loss = Some(loss);
                recovery.record_failure(Instant::now());
                counted_failures.device_loss(recovery.failure_epoch);
                return Ok(EncodeStep::Continue);
            }
            // The first frame at a size the client asked for failed:
            // go back to the mode the stream had, as when the encoder
            // cannot be created at it.
            if let Some((from, _)) = switching.take() {
                rollback_mode_switch(
                    thread,
                    c,
                    prepared,
                    stream,
                    resolution_cap,
                    from,
                    &error,
                    "reconfigure refused: the encoder failed at this mode; keeping the previous one",
                );
                *rebuild_encoder = true;
                return Ok(EncodeStep::Continue);
            }
            // A stalled or reset GPU costs these frames and a
            // keyframe; the rebuild requests it. Only failures
            // that keep coming end the session.
            let since = recovery.record_failure(Instant::now());
            if since.elapsed() >= ENCODER_RECOVERY {
                return Err(error.context(EncoderFailure("the encoder kept failing")));
            }
            s.launch.warnings.set("encoder_recovery", format!("Encoding failed ({error:#}); recreating the same encoder while the picture freezes. Lower game GPU load or update the graphics driver if this repeats."));
            active.log_stall();
            *rebuild_encoder = true;
            Ok(EncodeStep::Continue)
        }
    }
}

/// Evaluates a due `RFI_CONFIRM_KEYFRAME` timer: skips the confirming IDR when the client
/// actively proved clean post-RFI delivery (`0x5502` or `0x5547`) with zero follow-up requests,
/// and forces the confirming keyframe otherwise (including silent stock Moonlight clients).
pub(super) fn poll_rfi_confirmation(
    s: &Session,
    confirm_keyframe: &mut Option<Instant>,
    now: Instant,
) {
    if confirm_keyframe.is_some_and(|at| now >= at) {
        *confirm_keyframe = None;
        if s.rfi_recovery_confirmed() {
            s.clear_rfi_confirmation();
        } else {
            s.request_confirming_keyframe();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use crate::video_send::recovery::{RecoveryFrame, report};
    use butterpollo_media_policy::congestion::TransportFeedback;
    use butterpollo_session::session::Role;

    fn fec_status(frame: u32, received_data: u16, received_parity: u16) -> [u8; 21] {
        let mut pkt = [0; 21];
        pkt[..4].copy_from_slice(&frame.to_be_bytes());
        for (offset, word) in [(4, 11u16), (10, 10), (12, 2)] {
            pkt[offset..offset + 2].copy_from_slice(&word.to_be_bytes());
        }
        pkt[14..16].copy_from_slice(&received_data.to_be_bytes());
        pkt[16..18].copy_from_slice(&received_parity.to_be_bytes());
        pkt[18] = 20;
        pkt[20] = 1;
        pkt
    }

    fn report_ltr_repair(session: &Session, config: &Config, frame: u32, lost: bool) {
        session
            .stats
            .video_frame
            .store(u64::from(frame) + 1, Ordering::Release);
        report(
            session,
            config,
            RecoveryFrame {
                frame,
                idr: false,
                after_invalidation: true,
                payload_bytes: 12_000,
                cap_bytes: None,
                cap_target_bytes: None,
                reencoded: false,
                fec_limited: false,
                wire_bytes: 14_000,
                pace_bps: 80_000_000,
                elapsed: Duration::from_millis(1),
                lost,
            },
        );
    }

    #[test]
    fn rfi_confirm_timer_fires_without_client_feedback_and_skips_when_fec_proves_clean() {
        let f = Fixture::new();
        let config = Config::default();

        // Silent client (stock Moonlight without post-RFI confirmation): IDR fires at 1 s.
        let silent = f.session(f.client(u32::MAX), Role::Stream);
        silent.idr.store(false, Ordering::Release);
        let t0 = Instant::now();
        let mut due = Some(t0 + RFI_CONFIRM_KEYFRAME);
        silent.begin_rfi_confirmation();
        report_ltr_repair(&silent, &config, 41, false);
        poll_rfi_confirmation(&silent, &mut due, t0 + Duration::from_millis(500));
        assert!(due.is_some() && !silent.idr.load(Ordering::Acquire));
        poll_rfi_confirmation(&silent, &mut due, t0 + RFI_CONFIRM_KEYFRAME);
        assert!(due.is_none() && silent.idr.load(Ordering::Acquire));

        // Client reporting clean 0x5502 after LTR repair + zero follow-up requests: skips IDR.
        let clean = f.session(f.client(u32::MAX), Role::Stream);
        clean.idr.store(false, Ordering::Release);
        let mut due = Some(t0 + RFI_CONFIRM_KEYFRAME);
        clean.begin_rfi_confirmation();
        // In-flight pre-repair loss/invalidation for frame 40 must not poison repair frame 41.
        clean.stats.video_frame.store(41, Ordering::Release);
        clean.record_fec_status(&fec_status(40, 7, 1));
        report_ltr_repair(&clean, &config, 41, false);
        clean.request_invalidation(38, 40);
        let _ = clean.invalidation.lock().unwrap().take();
        clean.stats.video_frame.store(43, Ordering::Release);
        clean.record_fec_status(&fec_status(41, 9, 2)); // FEC-recovered repair
        clean.record_fec_status(&fec_status(42, 10, 2)); // Clean post-repair frame
        poll_rfi_confirmation(&clean, &mut due, t0 + RFI_CONFIRM_KEYFRAME);
        assert!(due.is_none());
        assert!(
            !clean.idr.load(Ordering::Acquire),
            "clean post-RFI FEC must skip 1 s IDR"
        );

        // Follow-up 0x0301 at/after repair frame still forces confirming IDR.
        let followup = f.session(f.client(u32::MAX), Role::Stream);
        followup.idr.store(false, Ordering::Release);
        let mut due = Some(t0 + RFI_CONFIRM_KEYFRAME);
        followup.begin_rfi_confirmation();
        report_ltr_repair(&followup, &config, 41, false);
        followup.stats.video_frame.store(43, Ordering::Release);
        followup.record_fec_status(&fec_status(42, 10, 2));
        followup.request_invalidation(41, 42);
        let _ = followup.invalidation.lock().unwrap().take();
        poll_rfi_confirmation(&followup, &mut due, t0 + RFI_CONFIRM_KEYFRAME);
        assert!(followup.idr.load(Ordering::Acquire));
    }

    #[test]
    fn rfi_confirm_timer_honors_transport_feedback_clean_vs_packet_loss() {
        use rubylight_protocol::transport_feedback::NOT_RECEIVED;
        let f = Fixture::new();
        let config = Config::default();

        let rtp_pkt = |seq: u16| {
            let mut p = vec![0u8; 1024];
            p[0] = 0x80;
            p[2..4].copy_from_slice(&seq.to_be_bytes());
            p
        };

        // Clean 0x5547 transport feedback after LTR repair skips confirming IDR.
        let s = f.session(f.client(u32::MAX), Role::Stream);
        s.configure_congestion(true, 0, false);
        s.idr.store(false, Ordering::Release);
        // Prime ledger via initial report.
        assert!(
            s.record_transport_feedback(
                &TransportFeedback {
                    seq: 0,
                    base: 1,
                    stride: 1,
                    flags: 0,
                    late: 0,
                    base_arrival_us: 1_000,
                    entries: vec![0],
                }
                .encode()
                .unwrap()
            )
        );
        let t0 = Instant::now();
        let mut due = Some(t0 + RFI_CONFIRM_KEYFRAME);
        s.begin_rfi_confirmation();
        s.record_transport_sent(&[rtp_pkt(10)], 66);
        report_ltr_repair(&s, &config, 41, false);
        s.record_transport_sent(&[rtp_pkt(11)], 66);
        assert!(
            s.record_transport_feedback(
                &TransportFeedback {
                    seq: 1,
                    base: 10,
                    stride: 1,
                    flags: 0,
                    late: 0,
                    base_arrival_us: 2_000,
                    entries: vec![0, 125],
                }
                .encode()
                .unwrap()
            )
        );
        poll_rfi_confirmation(&s, &mut due, t0 + RFI_CONFIRM_KEYFRAME);
        assert!(!s.idr.load(Ordering::Acquire));

        // Lost packet (NOT_RECEIVED) in 0x5547 without 0x5502 keeps confirming IDR.
        let lost = f.session(f.client(u32::MAX), Role::Stream);
        lost.configure_congestion(true, 0, false);
        lost.idr.store(false, Ordering::Release);
        assert!(
            lost.record_transport_feedback(
                &TransportFeedback {
                    seq: 0,
                    base: 1,
                    stride: 1,
                    flags: 0,
                    late: 0,
                    base_arrival_us: 1_000,
                    entries: vec![0],
                }
                .encode()
                .unwrap()
            )
        );
        let mut due = Some(t0 + RFI_CONFIRM_KEYFRAME);
        lost.begin_rfi_confirmation();
        lost.record_transport_sent(&[rtp_pkt(20)], 66);
        report_ltr_repair(&lost, &config, 50, false);
        lost.record_transport_sent(&[rtp_pkt(21)], 66);
        assert!(
            lost.record_transport_feedback(
                &TransportFeedback {
                    seq: 1,
                    base: 20,
                    stride: 1,
                    flags: 0,
                    late: 0,
                    base_arrival_us: 2_000,
                    entries: vec![NOT_RECEIVED, 125],
                }
                .encode()
                .unwrap()
            )
        );
        poll_rfi_confirmation(&lost, &mut due, t0 + RFI_CONFIRM_KEYFRAME);
        assert!(lost.idr.load(Ordering::Acquire));
    }
}
