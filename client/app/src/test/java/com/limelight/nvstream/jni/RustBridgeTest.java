package com.limelight.nvstream.jni;

import org.junit.Test;

import static org.junit.Assert.*;

public class RustBridgeTest {
    private static final String SUNSHINE = "7.1.431.-1";

    @Test public void ordinaryStreamsStart() {
        assertNull(RustBridge.hostRefusal(SUNSHINE, 0x0101, false));
        assertNull(RustBridge.hostRefusal("7.1.450.0", 0x0001, false));
        // Input-only streams negotiate no codec.
        assertNull(RustBridge.hostRefusal(SUNSHINE, 0, true));
    }

    @Test public void unsupportedHostsAreRefused() {
        assertNotNull(RustBridge.hostRefusal("7.1.430.0", 0x0101, false));
        assertNotNull(RustBridge.hostRefusal("5.0.0", 0x0101, false));
        assertNotNull(RustBridge.hostRefusal(null, 0x0101, false));
        assertNotNull(RustBridge.hostRefusal("garbage", 0x0101, false));
        assertNotNull(RustBridge.hostRefusal(SUNSHINE, 0, false));
    }

    @Test public void everyRefusalNamesItsCauseAndWhatToDo() {
        String old = RustBridge.hostRefusal("7.1.430.0", 0x0101, false);
        assertTrue(old.contains("GFE 3.22"));
        assertTrue(old.contains("7.1.430.0"));
        assertTrue(old.contains("Update"));
        assertTrue(RustBridge.hostRefusal(SUNSHINE, 0, false).contains("codec"));
    }

    @Test public void theEngineSummaryNamesTheEngineOrWhyItCouldNotStart() {
        StreamEngine.record(null);
        assertEquals("Rust (nightjar-core)", StreamEngine.summary());
        assertEquals("Rust", StreamEngine.label());

        String refusal = RustBridge.hostRefusal("7.1.430.0", 0x0101, false);
        StreamEngine.record(refusal);
        assertEquals("Rust (nightjar-core) could not start the stream: " + refusal, StreamEngine.summary());
        StreamEngine.record(null);
    }

    @Test public void appVersionsCompareNumerically() {
        int[] min = {7, 1, 431};
        assertTrue(RustBridge.appVersionAtLeast("7.1.431", min));
        assertTrue(RustBridge.appVersionAtLeast("7.2", min));
        assertTrue(RustBridge.appVersionAtLeast("8.0.0.0", min));
        assertFalse(RustBridge.appVersionAtLeast("7.1", min));
        assertFalse(RustBridge.appVersionAtLeast("7.1.-1", min));
    }
}
