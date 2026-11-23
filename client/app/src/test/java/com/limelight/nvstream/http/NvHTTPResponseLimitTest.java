package com.limelight.nvstream.http;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPResponseLimitTest {
    private static String paddedXml(int targetBytes) {
        String prefix = "<root status_code=\"200\"><uniqueid>host-1</uniqueid>"
                + "<state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus><!--";
        String suffix = "--></root>";
        int pad = targetBytes - prefix.length() - suffix.length();
        if (pad < 0) throw new IllegalArgumentException("targetBytes too small: " + targetBytes);
        return prefix + "a".repeat(pad) + suffix;
    }

    @Test
    public void rejectsOversizedDeclaredAndChunkedDiscoveryResponses() throws Exception {
        int limit = NvHTTP.MAX_XML_RESPONSE_BYTES;
        byte[] over = paddedXml(limit + 1).getBytes(java.nio.charset.StandardCharsets.UTF_8);
        AtomicInteger mode = new AtomicInteger();
        try (TestHttpServer server = new TestHttpServer(req -> {
            switch (mode.get()) {
                case 0:
                    return TestHttpServer.Response.bytes(200, "text/xml; charset=utf-8", over);
                case 1:
                    return TestHttpServer.Response.bytes(200, "text/xml; charset=utf-8", over).chunked();
                default:
                    return TestHttpServer.Response.bytes(200, "text/xml; charset=utf-8", new byte[] {'<' })
                            .declaredContentLength(limit + 2L);
            }
        })) {
            NvHTTP http = server.newClient();
            for (int m = 0; m < 3; m++) {
                mode.set(m);
                assertThrows("mode " + m, IOException.class, () -> http.getServerInfo(false));
            }
        }
    }

    @Test
    public void acceptsBoundaryAndAllowsLargerBoundedAppLists() throws Exception {
        int limit = NvHTTP.MAX_XML_RESPONSE_BYTES;
        String appPrefix = "<root status_code=\"200\"><!--";
        String appSuffix = "--></root>";
        String okApp = appPrefix + "a".repeat(limit + 1 - appPrefix.length() - appSuffix.length()) + appSuffix;
        String overApp = appPrefix + "a".repeat(NvHTTP.MAX_APP_LIST_BYTES + 1 - appPrefix.length() - appSuffix.length())
                + appSuffix;
        String exactInfo = paddedXml(limit);

        try (TestHttpServer server = new TestHttpServer(req -> {
            if ("/serverinfo".equals(req.path)) {
                return TestHttpServer.Response.xml(200, exactInfo).chunked();
            }
            if (req.rawQuery.contains("big=1")) {
                return TestHttpServer.Response.xml(200, overApp).chunked();
            }
            return TestHttpServer.Response.xml(200, okApp).chunked();
        })) {
            NvHTTP http = server.newClient();
            assertEquals(limit, http.getServerInfo(false).length());
            assertEquals(limit + 1, http.getAppListRaw().length());

            try (TestHttpServer bigServer = new TestHttpServer(
                    req -> TestHttpServer.Response.xml(200, overApp).chunked())) {
                assertThrows(IOException.class, () -> bigServer.newClient().getAppListRaw());
            }
        }
    }
}
