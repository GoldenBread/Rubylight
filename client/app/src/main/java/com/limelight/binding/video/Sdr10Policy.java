package com.limelight.binding.video;

import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * The decisions around a 10-bit SDR stream, apart from the Android objects so they can be tested
 * with made-up capabilities. There is no 10-bit SDR flag in the protocol: offering a 10-bit
 * format (without asking for HDR in the launch) is how the PC is asked to encode 10-bit BT.709.
 * It is offered only when HDR is not streamed, the setting is on, and the device decodes plain
 * Main10 in HEVC or AV1. PyroWave's 10-bit formats are an HDR10 swapchain here, so they are never
 * offered for this. A PC whose own display is in HDR still sends HDR10; that is named, not hidden.
 */
public final class Sdr10Policy {
    private Sdr10Policy() {}

    public enum Refusal {
        NONE,
        SETTING_OFF,
        /** HDR is being streamed; it wins and this is not needed. */
        HDR_STREAM,
        /** Input-only and control tiles have no video to improve. */
        NO_VIDEO,
        /** Neither HEVC nor AV1 has a Main10 decoder on this device (or the codec setting leaves them out). */
        NO_MAIN10,
        /** PyroWave is chosen, and its 10-bit output is the HDR10 path. */
        PYROWAVE,
        /** The experimental packed 4:4:4 path has its own 10-bit handling. */
        PACKED_444
    }

    private static int triBool(Boolean value) {
        return value == null ? -1 : (value ? 1 : 0);
    }

    public static Refusal refusal(boolean mediaStream, boolean enabled, boolean hdrGranted, boolean hevcMain10,
                                  boolean av1Main10, boolean pyroWaveChosen, boolean packed444) {
        return Refusal.values()[NightjarPolicyJni.sdr10Refusal(
                mediaStream, enabled, hdrGranted, hevcMain10, av1Main10, pyroWaveChosen, packed444)];
    }

    /** Whether the user should be told why an enabled setting is not used for this stream. */
    public static boolean worthSaying(Refusal refusal) {
        return NightjarPolicyJni.sdr10WorthSaying(refusal == null ? -1 : refusal.ordinal());
    }

    /** Whether the setting could change which decoders are used: only for a stream that may offer it. */
    public static boolean decoderChoiceMayChange(boolean enabled, boolean hdrRequested, boolean pyroWaveChosen,
                                                 boolean packed444) {
        return NightjarPolicyJni.sdr10DecoderChoiceMayChange(enabled, hdrRequested, pyroWaveChosen, packed444);
    }

    /**
     * Whether native 4:4:4 takes the stream from 10-bit SDR: the PC takes 4:4:4 or 10-bit, not both,
     * and the pick order ranks Main10 above 4:4:4, so this is settled before negotiating. Only a
     * 4:4:4 format the PC also supports counts.
     *
     * @param commonFormats the offered formats the PC also supports
     */
    public static boolean yuv444Wins(int commonFormats) {
        return NightjarPolicyJni.sdr10Yuv444Wins(commonFormats);
    }

    /** The offered formats without the 10-bit ones, which is how 4:4:4 wins over 10-bit SDR. */
    public static int withoutTenBit(int offeredFormats) {
        return NightjarPolicyJni.sdr10WithoutTenBit(offeredFormats);
    }

    /** What the HDR-in-SDR notice does on a status check. */
    public enum HostHdrStep { NONE, SAY, REARM }

    /**
     * The PC's HDR report on a 10-bit SDR stream: say it once when it turns to HDR, and arm the
     * notice again once it has turned back to SDR.
     *
     * @param previous the report seen at the last check, null before any
     */
    public static HostHdrStep hostHdrStep(boolean tenBit, Boolean hostHdr, Boolean previous) {
        return HostHdrStep.values()[NightjarPolicyJni.sdr10HostHdrStep(tenBit, triBool(hostHdr), triBool(previous))];
    }

    /** The 10-bit formats to offer in addition to the 8-bit ones: 0 unless the refusal is NONE. */
    public static int formats(Refusal refusal, boolean hevcMain10, boolean av1Main10) {
        return NightjarPolicyJni.sdr10Formats(refusal == null ? -1 : refusal.ordinal(), hevcMain10, av1Main10);
    }

    /** What the running 10-bit SDR stream says about colour, for the overlay's status line. */
    public enum Status {
        QUIET,
        /** The PC reports its picture as HDR: its display is in HDR, so the stream is not SDR. */
        HOST_HDR
    }

    /**
     * @param tenBit the negotiated format is 10-bit (8-bit is told once per PC when it is negotiated)
     * @param hostHdr the host's HDR report (setHdrMode), null before it arrives
     */
    public static Status status(boolean tenBit, Boolean hostHdr) {
        return Status.values()[NightjarPolicyJni.sdr10Status(tenBit, triBool(hostHdr))];
    }

    /**
     * The colour word of the overlay's stream line. A 10-bit stream is assumed HDR until the PC's
     * first report, unless it was asked for as 10-bit SDR.
     */
    public static String overlayColor(boolean tenBit, boolean sdr10, Boolean hostHdr) {
        return NightjarPolicyJni.sdr10OverlayColor(tenBit, sdr10, triBool(hostHdr));
    }

    /** Whether the decoder starts out configured for HDR before the PC has reported anything. */
    public static boolean assumeHdr(boolean tenBit, boolean sdr10) {
        return NightjarPolicyJni.sdr10AssumeHdr(tenBit, sdr10);
    }
}
