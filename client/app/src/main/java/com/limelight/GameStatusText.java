package com.limelight;

import android.content.Context;

import com.limelight.binding.video.DecoderSetupFailure;
import com.limelight.binding.video.HdrFormatPolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.utils.StreamHealth;
import com.limelight.utils.WifiDiagnostics;

import java.math.BigDecimal;
import java.util.Locale;

/**
 * Pure text and status formatters extracted from {@link Game}: stream health reports, rate-change
 * outcomes, startup/termination descriptions, HDR/renderer notice mappings, and post-stream
 * latency summary formatting.
 */
final class GameStatusText {
    private GameStatusText() {}

    /** The stream doctor's findings, evidence and connection facts; shared with the debug fixture. */
    static StringBuilder healthText(Context context, StreamHealth.Report report, String connection) {
        StringBuilder text = new StringBuilder(context.getString(R.string.health_intro, report.samples));
        if (!report.recent || report.samples < 3) text.append("\n\n").append(context.getString(R.string.health_insufficient));
        else if (report.findings.isEmpty()) text.append("\n\n").append(context.getString(R.string.health_no_issue));
        int[] descriptions = {R.string.health_network, R.string.health_decoder, R.string.health_host,
                R.string.health_presentation, R.string.health_heat};
        for (StreamHealth.Finding finding : report.findings) {
            text.append("\n\n").append(finding == StreamHealth.Finding.PERIODIC ?
                    context.getString(R.string.health_periodic,
                            WifiDiagnostics.period(context, report.stallPeriodMs), report.stalls) :
                    finding == StreamHealth.Finding.BURSTS ?
                    context.getString(R.string.health_bursts, report.lateSamples, report.samples) :
                    context.getString(descriptions[finding.ordinal()]));
        }
        text.append("\n\n").append(context.getString(
                report.hostIncludesSend ? R.string.health_evidence_send : R.string.health_evidence,
                healthMeasurement(context, report.worstLossPercent), healthMeasurement(context, report.worstDecodeMs),
                healthMeasurement(context, report.worstHostP95Ms), healthMeasurement(context, report.worstDisplayP95Ms)));
        text.append("\n\n").append(context.getString(R.string.wifi_heading)).append('\n').append(connection);
        return text;
    }

    private static String healthMeasurement(Context context, float value) {
        return Float.isFinite(value) && value >= 0 ? String.format(Locale.getDefault(), "%.1f", value) :
                context.getString(R.string.latency_unavailable);
    }

    /**
     * The doctor's bitrate trial runs only while nothing automatic owns the bitrate: the host's
     * adaptive bitrate, automatic bitrate on this device (0x553C) or adaptive quality.
     */
    static boolean healthTrialAllowed(boolean adaptiveBitrate, boolean rateChangeBitrate, boolean adaptiveQuality) {
        return !adaptiveBitrate && !rateChangeBitrate && !adaptiveQuality;
    }

    // What became of each value asked for, in plain words.
    static String rateChangeText(Context context, RateChangeResult result) {
        StringBuilder text = new StringBuilder();
        double mbps = result.bitrateKbps / 1000.0;
        switch (result.bitrateStatus) {
            case RateChangeResult.STATUS_APPLIED:
                text.append(context.getString(R.string.stream_rate_change_bitrate_applied, mbps));
                break;
            case RateChangeResult.STATUS_CLAMPED:
                text.append(context.getString(R.string.stream_rate_change_bitrate_clamped, mbps,
                        rateChangeReason(context, result.bitrateReason)));
                break;
            case RateChangeResult.STATUS_REFUSED:
                text.append(context.getString(R.string.stream_rate_change_bitrate_refused, mbps,
                        rateChangeReason(context, result.bitrateReason)));
                break;
            default:
                break;
        }
        String fps = BigDecimal.valueOf(result.fpsMillihz, 3).stripTrailingZeros().toPlainString();
        int fpsText = result.fpsStatus == RateChangeResult.STATUS_APPLIED ? R.string.stream_rate_change_fps_applied :
                result.fpsStatus == RateChangeResult.STATUS_CLAMPED ? R.string.stream_rate_change_fps_clamped :
                result.fpsStatus == RateChangeResult.STATUS_REFUSED ? R.string.stream_rate_change_fps_refused : 0;
        if (fpsText != 0) {
            if (text.length() > 0) text.append('\n');
            text.append(fpsText == R.string.stream_rate_change_fps_applied ? context.getString(fpsText, fps) :
                    context.getString(fpsText, fps, rateChangeReason(context, result.fpsReason)));
        }
        return text.toString();
    }

    static String rateChangeReason(Context context, int reason) {
        switch (reason) {
            case RateChangeResult.REASON_INVALID: return context.getString(R.string.stream_rate_change_reason_invalid);
            case RateChangeResult.REASON_DISABLED: return context.getString(R.string.stream_rate_change_reason_disabled);
            case RateChangeResult.REASON_CODEC: return context.getString(R.string.stream_rate_change_reason_codec);
            case RateChangeResult.REASON_HOST_LIMIT: return context.getString(R.string.stream_rate_change_reason_host_limit);
            case RateChangeResult.REASON_ENCODER_LIMIT: return context.getString(R.string.stream_rate_change_reason_encoder_limit);
            case RateChangeResult.REASON_ENCODER_FAILED: return context.getString(R.string.stream_rate_change_reason_encoder_failed);
            case RateChangeResult.REASON_SUPERSEDED: return context.getString(R.string.stream_rate_change_reason_superseded);
            default: return context.getString(R.string.stream_rate_change_reason_other);
        }
    }

    /** A recorded decoder setup failure at a video stage: this device's failure, not the host's. */
    static boolean isDecoderStageFailure(DecoderSetupFailure decoderFailure, String stage) {
        return decoderFailure != null && stage.contains("video");
    }

    /** The stream dialog's text for a failed stage: the decoder's reason, else the connection error, else the host's. */
    static String startupFailureText(Context context, DecoderSetupFailure decoderFailure, String stage,
                                     String connectionErrorMessage, int errorCode) {
        if (isDecoderStageFailure(decoderFailure, stage)) {
            return decoderFailure.message(context);
        }
        if (connectionErrorMessage != null) {
            return connectionErrorMessage;
        }
        return context.getString(errorCode == 0 ? R.string.startup_connection_failed :
                R.string.startup_host_error, errorCode);
    }

    static String terminationDescription(int errorCode) {
        String code = Math.abs(errorCode) > 1000 ? "0x" + Integer.toHexString(errorCode) : Integer.toString(errorCode);
        switch (errorCode) {
            case MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC: return code + " (no video traffic)";
            case MoonBridge.ML_ERROR_NO_VIDEO_FRAME: return code + " (no complete video frame)";
            case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION: return code + " (ended right after starting)";
            case MoonBridge.ML_ERROR_PROTECTED_CONTENT: return code + " (protected content)";
            case MoonBridge.ML_ERROR_FRAME_CONVERSION: return code + " (frame conversion failed on the PC)";
            case MoonBridge.CLIENT_ERROR_VULKAN_RENDERER: return code + " (Vulkan renderer failed on this device)";
            case MoonBridge.CLIENT_ERROR_PACKED_444_REFUSED: return code + " (full chroma not available with this PC)";
            case MoonBridge.CLIENT_ERROR_HOST_GONE: return code + " (the PC is gone)";
            case MoonBridge.CLIENT_ERROR_ENGINE_UNAVAILABLE: return code + " (the streaming engine could not start this stream)";
            case MoonBridge.CLIENT_ERROR_DECODER: return code + " (video decoder failed on this device)";
            // This device's own code for a connection that ended without any reason from the PC.
            case -1: return code + " (connection lost, no reason from the PC)";
            default: return code + " (reported by the PC)";
        }
    }

    /** Large codes are hex values, small ones decimal. */
    static String errorCodeText(int errorCode) {
        return Math.abs(errorCode) > 1000 ? Integer.toHexString(errorCode) : Integer.toString(errorCode);
    }

    static int hdrStatusText(HdrPolicy.Status status, boolean pyroWaveHostUpdate, boolean pyroWaveHlgRefused) {
        if (status == HdrPolicy.Status.PQ_ON_HLG_SCREEN && pyroWaveHostUpdate) {
            return R.string.stream_hdr_pq_on_hlg_screen_host_update;
        }
        if (status == HdrPolicy.Status.PQ_ON_HLG_SCREEN && pyroWaveHlgRefused) {
            return R.string.stream_hdr_pq_on_hlg_screen_display_refused;
        }
        switch (status) {
            case NOT_NEGOTIATED: return R.string.stream_hdr_not_negotiated;
            case HOST_SDR: return R.string.stream_hdr_host_sdr;
            case NO_METADATA: return R.string.stream_hdr_no_metadata;
            case OUTPUT_NOT_PQ: return R.string.stream_hdr_output_not_pq;
            case PQ_ON_HLG_SCREEN: return R.string.stream_hdr_pq_on_hlg_screen;
            case SHOWN_AS_SDR: return R.string.stream_hdr_vulkan_shown_as_sdr;
            default: return 0;
        }
    }

    /** The once-per-PC words for a Vulkan renderer limit, naming Frame Warp when it brought the renderer. */
    static int vulkanHdrLimitText(HdrFormatPolicy.DrawnLimit limit, boolean forFrameWarp) {
        switch (limit) {
            case DOLBY_VISION: return forFrameWarp ? R.string.stream_hdr_frame_warp_no_dolby_vision :
                    R.string.stream_hdr_vulkan_no_dolby_vision;
            default: return 0;
        }
    }

    /** The once-per-PC words for a PyroWave limit; the Dolby Vision text takes HDR10 or HLG. */
    static int pyroWaveHdrLimitText(HdrFormatPolicy.DrawnLimit limit) {
        switch (limit) {
            case DOLBY_VISION: return R.string.stream_hdr_pyrowave_no_dolby_vision;
            case HDR10_PLUS: return R.string.stream_hdr_pyrowave_no_hdr10_plus;
            default: return 0;
        }
    }

    static String latencyToast(Context context, int videoFormat, int averageEndToEndLat, int averageDecoderLat,
                               boolean streamSdr10, Boolean hostHdrReport) {
        String message = null;
        if (averageEndToEndLat > 0) {
            message = context.getResources().getString(R.string.conn_client_latency) + " " + averageEndToEndLat + " ms";
            if (averageDecoderLat > 0) {
                message += " (" + context.getResources().getString(R.string.conn_client_latency_hw) + " " + averageDecoderLat + " ms)";
            }
        } else if (averageDecoderLat > 0) {
            message = context.getResources().getString(R.string.conn_hardware_latency) + " " + averageDecoderLat + " ms";
        }
        if (message == null) return null;

        message += " [";
        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0) {
            message += "H.264";
        } else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) {
            message += "HEVC";
        } else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) {
            message += "AV1";
        } else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            message += "PyroWave";
        } else {
            message += "UNKNOWN";
        }

        if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0) {
            message += streamSdr10 && !Boolean.TRUE.equals(hostHdrReport) ? " 10-bit SDR" : " HDR";
        }
        return message + "]";
    }
}
