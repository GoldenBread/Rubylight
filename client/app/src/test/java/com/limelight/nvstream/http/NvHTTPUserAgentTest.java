package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.BuildConfig;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPUserAgentTest {
    private static final String EMPTY = "{\"version\":1,\"sampled_at_unix_ms\":1791712800000," +
            "\"interval_ms\":1000,\"cpu\":{},\"gpus\":[]}";

    private static final String METRICS_SERVERINFO = "<root status_code=\"200\"><uniqueid>host-1</uniqueid>" +
            "<state>SUNSHINE_SERVER_FREE</state><HttpsPort>47984</HttpsPort>" +
            "<RustHostVersion>3.1.0</RustHostVersion><PairStatus>1</PairStatus><appversion>7.1.0.0</appversion>" +
            "<HardwareMetricsVersion>1</HardwareMetricsVersion><Permission>" + ComputerDetails.PERMISSION_VIEW + "</Permission></root>";

    @Test
    public void theUserAgentNamesTheAppVersion() {
        assertEquals("Nightjar/" + BuildConfig.VERSION_NAME, NvHTTP.USER_AGENT);
        assertTrue(NvHTTP.USER_AGENT, NvHTTP.USER_AGENT.matches("Nightjar/\\d+\\.\\d+\\.\\d+\\S*"));
    }

    @Test
    public void everyClientToTheHostSendsTheUserAgent() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> {
            if ("/serverinfo".equals(req.path)) {
                return TestHttpServer.Response.xml(200, METRICS_SERVERINFO);
            }
            return TestHttpServer.Response.json(200, EMPTY);
        })) {
            NvHTTP http = server.newClient();
            http.getComputerDetails(METRICS_SERVERINFO);
            http.getServerInfo(true);
            http.getHardwareMetrics();
            assertEquals(2, server.requests().size());
            for (TestHttpServer.RecordedRequest req : server.requests()) {
                assertEquals(NvHTTP.USER_AGENT, req.header("User-Agent"));
            }
        }
    }

    @Test
    public void aCallThatBuildsItsOwnClientKeepsTheUserAgent() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.json(200, EMPTY))) {
            NvHTTP http = server.newClient();
            http.getComputerDetails(METRICS_SERVERINFO);
            http.getHardwareMetrics();
            assertEquals(1, server.requests().size());
            assertEquals(NvHTTP.USER_AGENT, server.requests().get(0).header("User-Agent"));
        }
    }
}
