package com.limelight.nvstream.jni;

/**
 * The connection stage names and the port flags a failed stage or end implicates, which
 * moonlight-common-c answered (LiGetStageName, LiGetPortFlagsFromStage,
 * LiGetPortFlagsFromTerminationErrorCode, LiStringifyPortFlags). Plain Java, so reports and tests
 * read them without a native library. nightjar-core's Stage numbers them the same way.
 */
public final class PortFlags {
    private PortFlags() {}

    /** The stage numbers (STAGE_*) and what the connecting screen calls them, in order. */
    private static final String[] STAGE_NAMES = {
            "none",
            "platform initialization",
            "name resolution",
            "audio stream initialization",
            "RTSP handshake",
            "control stream initialization",
            "video stream initialization",
            "input stream initialization",
            "control stream establishment",
            "video stream establishment",
            "audio stream establishment",
            "input stream establishment",
    };

    private static final int STAGE_RTSP_HANDSHAKE = 4;
    private static final int STAGE_CONTROL_STREAM_START = 8;

    /** The port flags with a port (ML_PORT_FLAG_*, without ALL). */
    static final int VALID_MASK = MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_TCP_47989
            | MoonBridge.ML_PORT_FLAG_TCP_48010 | MoonBridge.ML_PORT_FLAG_UDP_47998
            | MoonBridge.ML_PORT_FLAG_UDP_47999 | MoonBridge.ML_PORT_FLAG_UDP_48000
            | MoonBridge.ML_PORT_FLAG_UDP_48010;

    /** The query parameters every launch and resume URL carries: the control stream encryption generation. */
    public static final String LAUNCH_URL_QUERY_PARAMETERS = "&corever=1";

    /** The stage's name, or "unknown" for a number outside the stages. */
    public static String stageName(int stage) {
        return stage >= 0 && stage < STAGE_NAMES.length ? STAGE_NAMES[stage] : "unknown";
    }

    /** The ports a failure in this stage points at, 0 for a stage no port blocks. */
    public static int fromStage(int stage) {
        switch (stage) {
            case STAGE_RTSP_HANDSHAKE:
                // GFE 3.22 requires a successful ping on 48000 to complete the RTSP handshake.
                return MoonBridge.ML_PORT_FLAG_TCP_48010 | MoonBridge.ML_PORT_FLAG_UDP_48010
                        | MoonBridge.ML_PORT_FLAG_UDP_48000;
            case STAGE_CONTROL_STREAM_START:
                return MoonBridge.ML_PORT_FLAG_UDP_47999;
            default:
                return 0;
        }
    }

    /** The ports a connection that ended with this error points at, 0 for a code no port explains. */
    public static int fromTerminationErrorCode(int errorCode) {
        if (errorCode == MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC) {
            // Video is UDP 47998; 48000 too, as there is no audio traffic error.
            return MoonBridge.ML_PORT_FLAG_UDP_47998 | MoonBridge.ML_PORT_FLAG_UDP_48000;
        }
        return 0;
    }

    /** "TCP 47984", "UDP 47998", ... for each flag set, in flag order, joined by {@code separator} (null: none). */
    public static String stringify(int portFlags, String separator) {
        if (separator == null) separator = "";
        StringBuilder out = new StringBuilder();
        for (int i = 0; i < 32; i++) {
            if ((portFlags & VALID_MASK & (1 << i)) == 0) continue;
            if (out.length() > 0) out.append(separator);
            out.append(i >= 8 ? "UDP " : "TCP ").append(port(i));
        }
        return out.toString();
    }

    private static int port(int index) {
        switch (index) {
            case 0: return 47984;
            case 1: return 47989;
            case 2: return 48010;
            case 8: return 47998;
            case 9: return 47999;
            case 10: return 48000;
            default: return 48010;
        }
    }
}
