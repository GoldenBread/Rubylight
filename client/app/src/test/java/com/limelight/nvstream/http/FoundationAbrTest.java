package com.limelight.nvstream.http;

import android.app.Application;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.Assert.*;

// Foundation Sunshine's /api/abr shapes (src/nvhttp/abr_api.cpp), as Moonlight V+ sends them;
// the same as protocol/client/src/http/foundation.rs on desktop.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FoundationAbrTest {
    @Test
    public void requestsAreVPlusBodies() throws Exception {
        JSONObject on = FoundationAbr.enableRequest(40000);
        assertEquals(4, on.length());
        assertTrue(on.getBoolean("enabled"));
        assertEquals("balanced", on.getString("mode"));
        assertEquals(0, on.getInt("minBitrate"));
        assertEquals(40000, on.getInt("maxBitrate"));
        JSONObject off = FoundationAbr.disableRequest();
        assertEquals(1, off.length());
        assertFalse(off.getBoolean("enabled"));
    }

    @Test
    public void answersReadAsFoundationWritesThem() throws Exception {
        assertTrue(FoundationAbr.supported("{\"supported\":true,\"version\":1,\"features\":[\"llm_ai\"],\"llmEnabled\":false,\"hostMaxBitrate\":80000}"));
        assertFalse(FoundationAbr.supported("{\"supported\":false}"));
        assertEquals(35000, FoundationAbr.initialBitrate("{\"success\":true,\"enabled\":true,\"mode\":\"balanced\",\"minBitrate\":0," +
                "\"maxBitrate\":40000,\"initialBitrate\":35000,\"bitrateApplied\":true}"));
        IOException refused = assertThrows(IOException.class, () ->
                FoundationAbr.initialBitrate("{\"success\":false,\"error\":\"No active streaming session for this client\"}"));
        assertEquals("No active streaming session for this client", refused.getMessage());
        assertThrows(IOException.class, () -> FoundationAbr.supported("not json"));
    }

    // Foundation's network_probe.cpp capabilities, validated as V+ Android does via nightjar-core.
    @Test
    public void theBandwidthTestSizeFollowsThePcsCapabilities() throws Exception {
        AtomicReference<String> capsJson = new AtomicReference<>();
        AtomicInteger requestedBytes = new AtomicInteger();
        try (TestHttpServer server = new TestHttpServer(req -> {
            if ("/api/network/capabilities".equals(req.path)) {
                return TestHttpServer.Response.json(200, capsJson.get());
            }
            if ("/api/network/probe".equals(req.path)) {
                int len = Integer.parseInt(req.queryParam("bytes"));
                requestedBytes.set(len);
                return TestHttpServer.Response.bytes(200, "application/octet-stream", new byte[len])
                        .header("X-Bandwidth-Probe-Version", "1")
                        .header("X-Bandwidth-Probe-Nonce", req.queryParam("nonce"));
            }
            return TestHttpServer.Response.text(404, "not found");
        })) {
            NvHTTP http = server.newClient();
            capsJson.set("{\"version\":1,\"features\":[\"bandwidth-probe-v1\"]," +
                    "\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":262144,\"maxBytes\":8388608,\"cooldownMs\":10000}}");
            assertTrue(http.supportsFoundationBandwidthTest());
            http.probeFoundationBandwidth(percent -> { });
            assertEquals(4 * 1024 * 1024, requestedBytes.get());

            capsJson.set("{\"version\":1,\"bandwidthProbe\":{\"version\":1," +
                    "\"endpoint\":\"/api/network/probe\",\"minBytes\":1000,\"maxBytes\":2000}}");
            assertTrue(http.supportsFoundationBandwidthTest());
            http.probeFoundationBandwidth(percent -> { });
            assertEquals(2000, requestedBytes.get());

            capsJson.set("{\"version\":1}");
            assertFalse(http.supportsFoundationBandwidthTest());

            for (String bad : new String[] {
                    "{\"version\":0,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":1,\"maxBytes\":2}}",
                    "{\"version\":1,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/elsewhere\",\"minBytes\":1,\"maxBytes\":2}}",
                    "{\"version\":1,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":3,\"maxBytes\":2}}",
                    "{\"version\":1,\"bandwidthProbe\":{\"version\":1,\"endpoint\":\"/api/network/probe\",\"minBytes\":8388608,\"maxBytes\":9000000}}",
            }) {
                capsJson.set(bad);
                assertThrows(bad, IOException.class, () -> http.probeFoundationBandwidth(percent -> { }));
            }
        }
    }

    @Test
    public void reportsGoEveryThreeSecondsOneAtATime() throws Exception {
        try (FoundationAbr abr = new FoundationAbr(35000, 50000, 1000);
             FoundationAbr zero = new FoundationAbr(0, 50000, 0)) {
            assertEquals(35000, abr.currentKbps());
            assertEquals(50000, zero.currentKbps());
            for (int i = 0; i < 3; i++) abr.add(2.5, 117);
            assertNull("not before three seconds", abr.due(3999, 8));
            JSONObject report = abr.due(4000, 8);
            assertEquals(5, report.length());
            assertEquals(2.5, report.getDouble("packetLoss"), 1e-9);
            assertEquals(8.0, report.getDouble("rttMs"), 1e-9);
            assertEquals(117.0, report.getDouble("decodeFps"), 1e-9);
            assertEquals(9, report.getInt("droppedFrames"));
            assertEquals(35000, report.getInt("currentBitrate"));
            abr.add(0, 120);
            assertNull("the last report is not answered yet", abr.due(8000, 0));
            assertEquals(28000, abr.answered(new JSONObject("{\"newBitrate\":28000,\"bitrateApplied\":true,\"reason\":\"loss\"}")));
            JSONObject next = abr.due(8000, 0);
            assertEquals(28000, next.getInt("currentBitrate"));
            assertEquals(0.0, next.getDouble("packetLoss"), 1e-9);
            assertEquals(0, abr.answered(new JSONObject("{\"bitrateApplied\":true,\"reason\":\"stable\"}")));
            assertEquals("a failed report changes nothing", 0, abr.answered(null));
            assertEquals(28000, abr.currentKbps());
        }
    }

    @Test
    public void configureFoundationAbrSendsMinAndMaxBitrateOnWire() throws Exception {
        AtomicReference<String> lastBody = new AtomicReference<>();
        AtomicReference<String> responseBody = new AtomicReference<>(
                "{\"success\":true,\"enabled\":true,\"mode\":\"balanced\",\"minBitrate\":0,"
                        + "\"maxBitrate\":40000,\"initialBitrate\":35000,\"bitrateApplied\":true}");
        try (TestHttpServer server = new TestHttpServer(req -> {
            if ("POST".equals(req.method) && "/api/abr".equals(req.path)) {
                lastBody.set(req.bodyUtf8());
                return TestHttpServer.Response.json(200, responseBody.get());
            }
            return TestHttpServer.Response.text(404, "not found");
        })) {
            try (NvHTTP http = server.newClient()) {
                int initial = http.configureFoundationAbr(FoundationAbr.enableRequest(40000));
                assertEquals(35000, initial);
                JSONObject sent = new JSONObject(lastBody.get());
                assertTrue(sent.getBoolean("enabled"));
                assertEquals("balanced", sent.getString("mode"));
                assertEquals(0, sent.getInt("minBitrate"));
                assertEquals(40000, sent.getInt("maxBitrate"));

                responseBody.set("{\"success\":true,\"enabled\":false}");
                assertEquals(0, http.configureFoundationAbr(FoundationAbr.disableRequest()));
                JSONObject off = new JSONObject(lastBody.get());
                assertFalse(off.getBoolean("enabled"));
                assertFalse(off.has("minBitrate"));
                assertFalse(off.has("maxBitrate"));

                responseBody.set("{\"success\":false,\"error\":\"No active streaming session for this client\"}");
                IOException refused = assertThrows(IOException.class,
                        () -> http.configureFoundationAbr(FoundationAbr.enableRequest(40000)));
                assertTrue(refused.getMessage().contains("No active streaming session"));
            }
        }
    }
}
