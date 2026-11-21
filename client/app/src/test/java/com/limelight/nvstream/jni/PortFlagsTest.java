package com.limelight.nvstream.jni;

import org.junit.Test;

import static org.junit.Assert.*;

public class PortFlagsTest {
    @Test public void stageNamesAreTheOnesMoonlightCommonCUsed() {
        String[] expected = {"none", "platform initialization", "name resolution", "audio stream initialization",
                "RTSP handshake", "control stream initialization", "video stream initialization",
                "input stream initialization", "control stream establishment", "video stream establishment",
                "audio stream establishment", "input stream establishment"};
        for (int stage = 0; stage < expected.length; stage++) {
            assertEquals(expected[stage], PortFlags.stageName(stage));
        }
        assertEquals("unknown", PortFlags.stageName(12));
        assertEquals("unknown", PortFlags.stageName(-1));
    }

    @Test public void failedStagesNameTheirPorts() {
        assertEquals(MoonBridge.ML_PORT_FLAG_TCP_48010 | MoonBridge.ML_PORT_FLAG_UDP_48010 | MoonBridge.ML_PORT_FLAG_UDP_48000,
                PortFlags.fromStage(4));
        assertEquals(MoonBridge.ML_PORT_FLAG_UDP_47999, PortFlags.fromStage(8));
        for (int stage : new int[]{0, 1, 2, 3, 5, 6, 7, 9, 10, 11}) {
            assertEquals("stage " + stage, 0, PortFlags.fromStage(stage));
        }
    }

    @Test public void noVideoTrafficPointsAtVideoAndAudioPorts() {
        assertEquals(MoonBridge.ML_PORT_FLAG_UDP_47998 | MoonBridge.ML_PORT_FLAG_UDP_48000,
                PortFlags.fromTerminationErrorCode(MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC));
        assertEquals(0, PortFlags.fromTerminationErrorCode(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION));
        assertEquals(0, PortFlags.fromTerminationErrorCode(MoonBridge.ML_ERROR_NO_VIDEO_FRAME));
    }

    @Test public void stringifyListsEveryFlagInOrder() {
        assertEquals("", PortFlags.stringify(0, "\n"));
        assertEquals("TCP 47984", PortFlags.stringify(MoonBridge.ML_PORT_FLAG_TCP_47984, "\n"));
        assertEquals("TCP 48010\nUDP 47998\nUDP 48000",
                PortFlags.stringify(MoonBridge.ML_PORT_FLAG_UDP_48000 | MoonBridge.ML_PORT_FLAG_UDP_47998
                        | MoonBridge.ML_PORT_FLAG_TCP_48010, "\n"));
        assertEquals("TCP 47984, TCP 47989, TCP 48010, UDP 47998, UDP 47999, UDP 48000, UDP 48010",
                PortFlags.stringify(MoonBridge.ML_PORT_FLAG_ALL, ", "));
        assertEquals("TCP 47984UDP 48010",
                PortFlags.stringify(MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_UDP_48010, null));
    }

    @Test public void launchUrlCarriesTheCoreVersion() {
        assertEquals("&corever=1", PortFlags.LAUNCH_URL_QUERY_PARAMETERS);
    }
}
