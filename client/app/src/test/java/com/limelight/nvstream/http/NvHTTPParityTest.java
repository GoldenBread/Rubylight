package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.StreamConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.io.InputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import javax.crypto.spec.SecretKeySpec;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPParityTest {
    private NvHTTP http() throws IOException {
        return http("192.0.2.1", 47989);
    }

    private NvHTTP http(String address, int port) throws IOException {
        return new NvHTTP(new ComputerDetails.AddressTuple(address, port), 47984,
                "1234567890abcdef", null, new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        });
    }

    private String fixture(String name) throws Exception {
        try (InputStream input = getClass().getResourceAsStream("/protocol/" + name)) {
            assertNotNull(input);
            return new String(input.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    private ComputerDetails capabilities(String xml) throws Exception {
        ComputerDetails details = new ComputerDetails();
        NvHTTP.readClientCapabilities(details, xml);
        return details;
    }

    @Test
    public void cancellingAStreamAbortsAnIncompleteHttpBodyAndPreventsLaterRequests() throws Exception {
        ExecutorService workers = Executors.newFixedThreadPool(2);
        CountDownLatch headersSent = new CountDownLatch(1);
        CountDownLatch finish = new CountDownLatch(1);
        try (ServerSocket server = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            NvHTTP http = http(server.getInetAddress().getHostAddress(), server.getLocalPort());
            Future<?> host = workers.submit(() -> {
                try (Socket socket = server.accept()) {
                    java.io.BufferedReader reader = new java.io.BufferedReader(
                            new java.io.InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII));
                    while (!reader.readLine().isEmpty()) { }
                    socket.getOutputStream().write(("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n" +
                            "Content-Type: application/xml\r\n\r\n<root").getBytes(StandardCharsets.US_ASCII));
                    socket.getOutputStream().flush();
                    headersSent.countDown();
                    finish.await();
                } catch (Exception e) {
                    throw new RuntimeException(e);
                }
            });
            Future<String> response = workers.submit(() -> http.getServerInfo(true));
            try {
                assertTrue(headersSent.await(5, TimeUnit.SECONDS));
                http.cancelPendingRequests();
                ExecutionException failure = assertThrows(ExecutionException.class,
                        () -> response.get(2, TimeUnit.SECONDS));
                assertTrue(failure.getCause() instanceof IOException);
                assertThrows(IOException.class, () -> http.getServerInfo(true));
                assertThrows(IOException.class, http::probeServerLatency);
            } finally {
                finish.countDown();
            }
            host.get(2, TimeUnit.SECONDS);
        } finally {
            finish.countDown();
            workers.shutdownNow();
        }
    }

    // Nightjar #88: a launch waited for the PC's answer without any limit.
    @Test
    public void aLaunchThePcNeverAnswersTimesOutAsItsOwnFailure() throws Exception {
        assertTrue(NvHTTP.LAUNCH_READ_TIMEOUT_SECONDS * 1000L > NvHTTP.READ_TIMEOUT);
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(
                200, "<root status_code=\"200\"><sessionUrl0>rtsp://127.0.0.1:48010</sessionUrl0></root>")
                .delayMs(1400))) {
            NvHTTP http = server.newClient();
            ConnectionContext ctx = new ConnectionContext();
            ctx.streamConfig = new StreamConfiguration.Builder().build();
            ctx.negotiatedWidth = 1920;
            ctx.negotiatedHeight = 1080;
            ctx.riKey = new SecretKeySpec(new byte[16], "AES");
            ctx.hostCaps = HostCapabilities.UNKNOWN;
            NvHTTP.LaunchTimeoutException read = assertThrows(NvHTTP.LaunchTimeoutException.class,
                    () -> http.launchApp(ctx, "launch", 42, false, 1000L));
            assertEquals(1, read.seconds);
        }
        // A connect that refuses/fails stays the usual IOException.
        int unusedPort;
        try (ServerSocket s = new ServerSocket(0, 1, InetAddress.getLoopbackAddress())) {
            unusedPort = s.getLocalPort();
        }
        NvHTTP refused = new NvHTTP(new ComputerDetails.AddressTuple("127.0.0.1", unusedPort), unusedPort,
                "client", TestHttpServer.hostCert(), NativeApiFixtures.crypto());
        ConnectionContext ctx = new ConnectionContext();
        ctx.streamConfig = new StreamConfiguration.Builder().build();
        ctx.negotiatedWidth = 1920;
        ctx.negotiatedHeight = 1080;
        ctx.riKey = new SecretKeySpec(new byte[16], "AES");
        ctx.hostCaps = HostCapabilities.UNKNOWN;
        IOException connectErr = assertThrows(IOException.class,
                () -> refused.launchApp(ctx, "launch", 42, false, 1000L));
        assertFalse(connectErr instanceof NvHTTP.LaunchTimeoutException);
    }

    @Test
    public void latencyProbeHasABoundedDeadlineAndRejectsInvalidHostResponses() throws Exception {
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>(
                TestHttpServer.Response.xml(200, "<root status_code=\"200\"><uniqueid>host-1</uniqueid>" +
                        "<state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus><appversion>7.1.0.0</appversion></root>"));
        try (TestHttpServer server = new TestHttpServer(req -> {
            assertEquals("/serverinfo", req.path);
            return reply.get();
        })) {
            NvHTTP http = server.newClient();
            assertTrue(http.probeServerLatency() >= 0);
            reply.set(TestHttpServer.Response.xml(200, "<root status_code=\"200\" />"));
            assertThrows(org.xmlpull.v1.XmlPullParserException.class, http::probeServerLatency);
        }
    }

    @Test
    public void hardwareMetricsRequireSupportedDiscoveryAndPairedViewingPermission() throws Exception {
        ComputerDetails details = capabilities("<root status_code=\"200\"><RustHostVersion>2.2.5</RustHostVersion>" +
                "<PairStatus>1</PairStatus><HardwareMetricsVersion>1</HardwareMetricsVersion><Permission>" +
                ComputerDetails.PERMISSION_VIEW + "</Permission></root>");
        assertFalse(details.supportsHardwareMetrics());
        details.pairState = PairingManager.PairState.PAIRED;
        assertTrue(details.supportsHardwareMetrics());
        ComputerDetails copy = new ComputerDetails(details);
        assertTrue(copy.supportsHardwareMetrics());
        copy.permission = ComputerDetails.PERMISSION_INPUT;
        assertFalse(copy.supportsHardwareMetrics());
        copy.permission = ComputerDetails.PERMISSION_LAUNCH;
        assertTrue(copy.supportsHardwareMetrics());
        copy.caps = HostCapabilities.fromServerInfo("<root status_code=\"200\"><RustHostVersion>2.2.5</RustHostVersion>" +
                "<PairStatus>1</PairStatus></root>");
        assertFalse(copy.supportsHardwareMetrics());
        for (String version : new String[] {"0", "2", "-1", "invalid", ""}) {
            assertFalse(version, HostCapabilities.fromServerInfo("<root status_code=\"200\"><PairStatus>1</PairStatus>" +
                    "<RustHostVersion>2.2.5</RustHostVersion><HardwareMetricsVersion>" + version +
                    "</HardwareMetricsVersion></root>").has(HostCapabilities.Feature.HARDWARE_METRICS));
        }
        NvHTTP.readClientCapabilities(details, "<root status_code=\"200\" />");
        assertFalse(details.supportsHardwareMetrics());
        copy.update(details);
        assertFalse(copy.caps.has(HostCapabilities.Feature.HARDWARE_METRICS));
        assertFalse(copy.supportsHardwareMetrics());
    }

    @Test
    public void encoderBitratesAreCopiedAndMissingOrInvalidRatesStayUnknown() throws Exception {
        ComputerDetails details = capabilities("<root status_code=\"200\">" +
                "<RustHostEncoderBitrateKbps>30000</RustHostEncoderBitrateKbps>" +
                "<RustHostInitialEncoderBitrateKbps>38988</RustHostInitialEncoderBitrateKbps></root>");
        ComputerDetails copy = new ComputerDetails(details);
        assertTrue(copy.encoderBitrateSupported);
        assertEquals(30000, copy.encoderBitrateKbps);
        assertEquals(38988, copy.initialEncoderBitrateKbps);
        for (String value : new String[] {"", "0", "-1", "2000001", "invalid"}) {
            NvHTTP.readClientCapabilities(details, "<root status_code=\"200\">" +
                    "<RustHostEncoderBitrateKbps>" + value + "</RustHostEncoderBitrateKbps>" +
                    "<RustHostInitialEncoderBitrateKbps>" + value + "</RustHostInitialEncoderBitrateKbps></root>");
            assertTrue(details.encoderBitrateSupported);
            assertEquals(0, details.encoderBitrateKbps);
            assertEquals(0, details.initialEncoderBitrateKbps);
        }
        NvHTTP.readClientCapabilities(copy, "<root status_code=\"200\" />");
        assertFalse(copy.encoderBitrateSupported);
        assertEquals(0, copy.encoderBitrateKbps);
        assertEquals(0, copy.initialEncoderBitrateKbps);
        details.update(copy);
        assertFalse(details.encoderBitrateSupported);
        for (String field : new String[] {"RustHostEncoderBitrateKbps", "RustHostInitialEncoderBitrateKbps"}) {
            assertFalse(capabilities("<root status_code=\"200\"><" + field + ">0</" + field + "></root>")
                    .encoderBitrateSupported);
        }
    }

    @Test
    public void theHostsLastFailureIsReadAndClearedWhenItNoLongerSaysOne() throws Exception {
        ComputerDetails details = capabilities("<root status_code=\"200\">" +
                "<RustHostLastFailure> HDR is on, but the display is not in HDR &amp; mode </RustHostLastFailure></root>");
        assertEquals("HDR is on, but the display is not in HDR & mode", details.hostLastFailure);
        assertEquals(details.hostLastFailure, new ComputerDetails(details).hostLastFailure);
        NvHTTP.readClientCapabilities(details, "<root status_code=\"200\"><RustHostLastFailure></RustHostLastFailure></root>");
        assertEquals("", details.hostLastFailure);
        assertEquals("", capabilities("<root status_code=\"200\" />").hostLastFailure);
    }

    @Test
    public void rubylightAdvertisesUnsignedPermissionsAndFractionalLimiter() throws Exception {
        ComputerDetails details = http().getComputerDetails(fixture("butterpollo-serverinfo.xml"));
        assertEquals(0xFFFFFFFFL, details.permission);
        assertTrue(details.canReadClipboard());
        assertTrue(details.canWriteClipboard());
        assertTrue(details.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS));
        assertTrue(details.frameLimiterEnabled);
        assertTrue(details.virtualDisplayFrameLimiterEnabled);
        assertEquals(59940, details.frameLimiterFpsLimitMilliHz);
        assertArrayEquals(new byte[] {0}, details.serverCommandPayload(0));
    }

    @Test
    public void apolloCommandsPreserveEmptyNamesIndicesAndReservedBytes() throws Exception {
        ComputerDetails details = capabilities(fixture("apollo-serverinfo.xml"));
        assertEquals("Display & audio", details.serverCommands.get(0));
        assertEquals("", details.serverCommands.get(1));
        assertEquals("Sleep", details.serverCommands.get(2));
        assertArrayEquals(new byte[] {2, 0, 0, 0}, details.serverCommandPayload(2));
        assertFalse(details.canRunServerCommand(-1));
        assertFalse(details.canRunServerCommand(3));
        assertThrows(IllegalArgumentException.class, () -> details.serverCommandPayload(3));
    }

    @Test
    public void commandIndexCannotWrapBeyondOneByte() {
        ComputerDetails details = new ComputerDetails();
        for (int i = 0; i < 257; i++) {
            details.serverCommands.add("Command " + i);
        }
        assertArrayEquals(new byte[] {(byte) 255, 0, 0, 0}, details.serverCommandPayload(255));
        assertFalse(details.canRunServerCommand(256));
    }

    @Test
    public void viewerCanResumeButCannotLaunchQuitInputOrUseHostActions() throws Exception {
        ComputerDetails details = http().getComputerDetails(fixture("butterpollo-viewer-serverinfo.xml"));
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_LIST));
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH));
        assertFalse(details.hasPermission(ComputerDetails.PERMISSION_LAUNCH));
        assertFalse(details.hasPermission(ComputerDetails.PERMISSION_INPUT));
        assertFalse(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
        assertFalse(details.canRunServerCommand(0));
        assertFalse(details.frameLimiterEnabled);
        assertTrue(details.virtualDisplayFrameLimiterEnabled);
        assertEquals(0, details.frameLimiterFpsLimitMilliHz);
    }

    @Test
    public void clipboardDirectionsAreIndependentAndRequireViewing() throws Exception {
        ComputerDetails details = capabilities("<root status_code=\"200\"><Permission>" +
                (ComputerDetails.PERMISSION_LAUNCH | ComputerDetails.PERMISSION_CLIPBOARD_SET) + "</Permission></root>");
        assertTrue(details.canWriteClipboard());
        assertFalse(details.canReadClipboard());
        details.permission = ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_CLIPBOARD_READ;
        assertTrue(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
        details.permission = ComputerDetails.PERMISSION_CLIPBOARD_READ | ComputerDetails.PERMISSION_CLIPBOARD_SET;
        assertFalse(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
    }

    @Test
    public void stockSunshineKeepsNormalActionsWithoutApolloExtras() throws Exception {
        ComputerDetails details = capabilities(fixture("sunshine-serverinfo.xml"));
        assertEquals(-1, details.permission);
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_LAUNCH));
        assertTrue(details.hasPermission(ComputerDetails.PERMISSION_VIEW));
        assertFalse(details.canReadClipboard());
        assertFalse(details.canWriteClipboard());
        assertFalse(details.canRunServerCommand(0));
        assertFalse(details.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS));
    }

    @Test
    public void malformedPermissionsFailClosedWithoutBreakingDiscovery() throws Exception {
        for (String value : new String[] {"", "-1", "4294967296", "not-a-mask"}) {
            ComputerDetails details = capabilities("<root status_code=\"200\"><Permission>" + value +
                    "</Permission><ServerCommand>Sleep</ServerCommand></root>");
            assertEquals(0, details.permission);
            assertFalse(details.canRunServerCommand(0));
            assertFalse(details.hasPermission(ComputerDetails.PERMISSION_LAUNCH));
        }
    }

    @Test
    public void malformedHostNumbersUseCheckedErrorsOrOptionalFieldDefaults() throws Exception {
        NvHTTP http = http();
        for (String status : new String[] {"", "garbage", "4294967496", "-4294967096"}) {
            String xml = "<root status_code=\"" + status + "\"><appversion>7.1.2.3</appversion></root>";
            assertThrows(org.xmlpull.v1.XmlPullParserException.class, () -> http.getServerVersion(xml));
        }
        for (String value : new String[] {"-1", "65536", "garbage"}) {
            String xml = "<root status_code=\"200\"><HttpsPort>" + value +
                    "</HttpsPort><ExternalPort>" + value + "</ExternalPort></root>";
            assertEquals(47984, http.getHttpsPort(xml));
            assertEquals(47989, http.getExternalPort(xml));
        }
        for (String value : new String[] {"-1", "4294967296", "garbage"}) {
            assertEquals(0, http.getServerCodecModeSupport("<root status_code=\"200\"><ServerCodecModeSupport>" +
                    value + "</ServerCodecModeSupport></root>"));
        }
        assertThrows(org.xmlpull.v1.XmlPullParserException.class, () -> http.getCurrentGame(
                "<root status_code=\"200\"><state>SUNSHINE_SERVER_BUSY</state><currentgame>bad</currentgame></root>"));
        for (String version : new String[] {"7.1", "7.x.0.0", "7.1.-1.0", "7.1.431.-2"}) {
            assertThrows(org.xmlpull.v1.XmlPullParserException.class, () -> http.getServerAppVersionQuad(
                    "<root status_code=\"200\"><appversion>" + version + "</appversion></root>"));
        }
        assertArrayEquals(new int[] {7, 1, 431, -1}, http.getServerAppVersionQuad(
                "<root status_code=\"200\"><appversion>7.1.431.-1</appversion></root>"));
    }

    @Test
    public void stockUnsignedErrorStatusKeepsItsAudioDiagnostic() throws Exception {
        NvHTTP http = http();
        HostHttpResponseException error = assertThrows(HostHttpResponseException.class,
                () -> http.getServerVersion("<root status_code=\"4294967295\" status_message=\"Invalid\"/>"));
        assertEquals(418, error.getErrorCode());
    }

    @Test
    public void capabilityRefreshClearsOldGrantsAndDoesNotAliasCommands() throws Exception {
        ComputerDetails original = capabilities(fixture("butterpollo-serverinfo.xml"));
        ComputerDetails copy = new ComputerDetails(original);
        original.serverCommands.clear();
        assertTrue(copy.canRunServerCommand(0));
        copy.update(http().getComputerDetails(fixture("butterpollo-viewer-serverinfo.xml")));
        assertFalse(copy.canRunServerCommand(0));
        assertFalse(copy.canWriteClipboard());
        NvHTTP.readClientCapabilities(copy, fixture("sunshine-serverinfo.xml"));
        assertEquals(-1, copy.permission);
        assertEquals(HostCapabilities.Family.SUNSHINE, copy.caps.family());
        assertFalse(copy.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS));
    }

    @Test
    public void clipboardUsesHttpsPathPostBodyAndPerRequestIdentity() throws Exception {
        String text = "Grüße & <text>\nこんにちは 🧈";
        try (TestHttpServer server = new TestHttpServer(req -> {
            assertEquals("https", req.scheme);
            assertEquals("/actions/clipboard", req.path);
            assertEquals("text", req.queryParam("type"));
            assertEquals("1234567890abcdef", req.queryParam("uniqueid"));
            assertNotNull(req.queryParam("uuid"));
            assertFalse(req.rawQuery.contains("Grüße"));
            if ("POST".equals(req.method)) {
                assertEquals(text, req.bodyUtf8());
                assertTrue(req.header("content-type").startsWith("text/plain"));
                return TestHttpServer.Response.text(200, "");
            }
            assertEquals("GET", req.method);
            return TestHttpServer.Response.text(200, text);
        })) {
            NvHTTP http = server.newClient();
            http.sendClipboard(text);
            assertEquals(text, http.getClipboard());
        }
    }

    @Test
    public void clipboardRejectsXmlErrorPagesAndOversizedTransfersButAcceptsEmptyText() throws Exception {
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP http = server.newClient();
            reply.set(TestHttpServer.Response.text(200, ""));
            assertEquals("", http.getClipboard());
            reply.set(TestHttpServer.Response.xml(200, "<root status_code=\"404\"/>"));
            assertThrows(IOException.class, http::getClipboard);
            reply.set(TestHttpServer.Response.text(200, "a".repeat(1024 * 1024 + 1)));
            assertThrows(IOException.class, http::getClipboard);
            reply.set(TestHttpServer.Response.text(200, ""));
            http.sendClipboard("a".repeat(1024 * 1024));
            assertThrows(IOException.class, () -> http.sendClipboard("ä".repeat(524289)));
        }
    }

    @Test
    public void bandwidthProbeRequiresPairingAndBoundedAdvertisedCapability() throws Exception {
        ComputerDetails details = new ComputerDetails();
        for (String bytes : new String[] {"0", "-1", "33554433", "bad", "33554432"}) {
            NvHTTP.readClientCapabilities(details, "<root status_code=\"200\"><RustHostVersion>3.1.0</RustHostVersion>" +
                    "<PairStatus>1</PairStatus><PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>" +
                    bytes + "</PyroWaveBandwidthProbeBytes></root>");
            details.pairState = PairingManager.PairState.NOT_PAIRED;
            assertFalse(details.supportsPyroWaveBandwidthProbe());
            details.pairState = PairingManager.PairState.PAIRED;
            assertEquals("33554432".equals(bytes), details.supportsPyroWaveBandwidthProbe());
            assertEquals(2500, new ComputerDetails(details).pyroWaveHostLinkMbps);
        }
        NvHTTP client = http();
        assertThrows(IOException.class, () -> client.probePyroWaveBandwidth(details, percent -> {}));
        NvHTTP.readClientCapabilities(details, "<root status_code=\"200\"/>");
        assertFalse(details.supportsPyroWaveBandwidthProbe());
        assertEquals(0, details.pyroWaveHostLinkMbps);
    }

    @Test
    public void bandwidthProbeStreamsBoundedChunksAndReportsProgress() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> {
            assertEquals("/pyrowave-bandwidth-probe", req.path);
            return TestHttpServer.Response.bytes(200, "application/octet-stream", new byte[128 * 1024]);
        })) {
            List<Integer> progress = new ArrayList<>();
            server.newClient().probePyroWaveBandwidthBytes(128 * 1024, progress::add);
            assertFalse(progress.isEmpty());
            assertEquals(Integer.valueOf(100), progress.get(progress.size() - 1));
            int previous = -1;
            for (int percent : progress) {
                assertTrue(percent > previous && percent >= 0 && percent <= 100);
                previous = percent;
            }
        }
    }

    @Test
    public void bandwidthProbeRejectsErrorPagesAndAdvertisedLengthMismatch() throws Exception {
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP client = server.newClient();
            reply.set(TestHttpServer.Response.xml(200, "error"));
            assertThrows(IOException.class, () -> client.probePyroWaveBandwidthBytes(5, p -> {}));

            reply.set(TestHttpServer.Response.bytes(200, "application/octet-stream", new byte[20]));
            assertThrows(IOException.class, () -> client.probePyroWaveBandwidthBytes(10, p -> {}));
            assertThrows(IOException.class, () -> client.probePyroWaveBandwidthBytes(33554433, p -> {}));
        }
    }

    @Test
    public void bandwidthProbeRejectsTruncatedAndOversizedChunkedBodies() throws Exception {
        AtomicReference<byte[]> body = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response
                .bytes(200, "application/octet-stream", body.get()).chunked())) {
            NvHTTP client = server.newClient();
            for (int length : new int[] {9, 10, 11}) {
                body.set(new byte[length]);
                if (length == 10) {
                    client.probePyroWaveBandwidthBytes(10, p -> {});
                } else {
                    assertThrows("length " + length, IOException.class,
                            () -> client.probePyroWaveBandwidthBytes(10, p -> {}));
                }
            }
        }
    }

    @Test
    public void runtimeBitrateUsesKbpsAndReportsTheHostAppliedCap() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> {
            assertEquals("https", req.scheme);
            assertEquals("/bitrate", req.path);
            assertEquals("30000", req.queryParam("bitrate"));
            return TestHttpServer.Response.xml(200, "<root status_code=\"200\"><bitrate>20000</bitrate><updated>1</updated></root>");
        })) {
            NvHTTP http = server.newClient();
            assertEquals(20000, http.setBitrate(30000));
            assertThrows(IllegalArgumentException.class, () -> http.setBitrate(0));
            assertThrows(IllegalArgumentException.class, () -> http.setBitrate(2000001));
        }
    }

    @Test
    public void hostActionsPropagateRevocationUnsupportedEndpointsAndInvalidSuccessBodies() throws Exception {
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP http = server.newClient();
            for (int code : new int[] {403, 404, 500, 200}) {
                reply.set(TestHttpServer.Response.xml(code, "<root status_code=\"200\"><bitrate>0</bitrate></root>"));
                assertThrows(IOException.class, http::getClipboard);
                assertThrows(IOException.class, () -> http.sendClipboard("text"));
                assertThrows(IOException.class, () -> http.setBitrate(10000));
            }
        }
    }

    @Test
    public void requestNonceDoesNotReplaceThePersistentDeviceIdentity() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> {
            if ("/serverinfo".equals(req.path)) {
                return TestHttpServer.Response.xml(200,
                        "<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus><appversion>7.1.0.0</appversion></root>");
            }
            return TestHttpServer.Response.xml(200, "<root status_code=\"200\"><cancel>1</cancel></root>");
        })) {
            NvHTTP first = server.newClient("1234567890abcdef", true, NativeApiFixtures.crypto());
            NvHTTP second = server.newClient("fedcba0987654321", true, NativeApiFixtures.crypto());
            first.getServerInfo(true);
            first.quitApp();
            second.getServerInfo(true);
            List<TestHttpServer.RecordedRequest> reqs = server.requests();
            assertEquals("1234567890abcdef", reqs.get(0).queryParam("uniqueid"));
            assertEquals(reqs.get(0).queryParam("uniqueid"), reqs.get(1).queryParam("uniqueid"));
            assertNotEquals(reqs.get(0).queryParam("uuid"), reqs.get(1).queryParam("uuid"));
            assertNotEquals(reqs.get(0).queryParam("uniqueid"), reqs.get(reqs.size() - 1).queryParam("uniqueid"));
        }
    }

    @Test
    public void onlyTheSecondScreensProcessMarksEveryRequest() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> {
            if ("/api/network-path".equals(req.path)) {
                return TestHttpServer.Response.json(200, "{\"version\":1,\"path\":\"lan\"}");
            }
            if ("/api/hardware-metrics".equals(req.path)) {
                return TestHttpServer.Response.json(200,
                        "{\"version\":1,\"sampled_at_unix_ms\":1791712800000,\"interval_ms\":1000,\"cpu\":{},\"gpus\":[]}");
            }
            return TestHttpServer.Response.xml(200,
                    "<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus><bitrate>10000</bitrate><updated>1</updated><cancel>1</cancel></root>");
        })) {
            try {
                NvHTTP plain = server.newClient();
                plain.setBitrate(10000);
                NvHTTP.setSecondScreen(true);
                NvHTTP marked = server.newClient();
                marked.getComputerDetails("<root status_code=\"200\"><uniqueid>host-1</uniqueid>"
                        + "<state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus>"
                        + "<HardwareMetricsVersion>1</HardwareMetricsVersion>"
                        + "<RustHostVersion>3.1.0</RustHostVersion></root>");
                marked.setBitrate(10000);
                marked.quitApp();
                marked.getHardwareMetrics();
                List<TestHttpServer.RecordedRequest> reqs = server.requests();
                assertNull(reqs.get(0).queryParam(NvHTTP.SECOND_SCREEN_PARAMETER));
                for (int i = 1; i < reqs.size(); i++) {
                    assertEquals(Collections.singletonList("1"), reqs.get(i).queryValues(NvHTTP.SECOND_SCREEN_PARAMETER));
                    assertEquals("1234567890abcdef", reqs.get(i).queryParam("uniqueid"));
                }
            } finally {
                NvHTTP.setSecondScreen(false);
            }
            server.newClient().quitApp();
            List<TestHttpServer.RecordedRequest> reqs = server.requests();
            assertNull(reqs.get(reqs.size() - 1).queryParam(NvHTTP.SECOND_SCREEN_PARAMETER));
        }
    }

    @Test
    public void failedPairingUsesHttpBeforeRegistrationWhilePairedUnpairUsesHttps() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> {
            assertEquals("/unpair", req.path);
            assertEquals("1234567890abcdef", req.queryParam("uniqueid"));
            return TestHttpServer.Response.xml(200, "<root status_code=\"200\"><unpaired>1</unpaired></root>");
        })) {
            NvHTTP http = server.newClient(false);
            http.unpair();
            http.setServerCert(server.serverCert());
            http.unpair();
            assertEquals(Arrays.asList("http", "https"),
                    Arrays.asList(server.requests().get(0).scheme, server.requests().get(1).scheme));
        }
    }

    @Test
    public void oneTimePinPairingAddsOnlyAuthHashAndUsesReadTimeout() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(
                200, "<root status_code=\"200\"><paired>0</paired></root>"))) {
            for (String passphrase : new String[] {null, "one-time secret"}) {
                NvHTTP http = server.newClient("test-device", false, NativeApiFixtures.crypto());
                assertEquals(PairingManager.PairState.FAILED, http.getPairingManager().pair(
                        "<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>0</PairStatus><appversion>7.1.2.3</appversion></root>", "0042", passphrase));
                TestHttpServer.RecordedRequest req = server.requests().get(server.requests().size() - 1);
                assertEquals("getservercert", req.queryParam("phrase"));
                if (passphrase == null) {
                    assertNull(req.queryParam("otpauth"));
                } else {
                    assertTrue(req.queryParam("otpauth").matches("[0-9A-F]{64}"));
                    assertFalse(req.rawQuery.contains(passphrase));
                    assertNull(req.queryParam("pin"));
                }
            }
        }
    }

    @Test
    public void aLinkTestOnlyLowersTheFirstBitrate() {
        assertEquals(20000, NvHTTP.probeStartKbps(40000, 25));
        assertEquals(40000, NvHTTP.probeStartKbps(40000, 500));
        assertEquals(40000, NvHTTP.probeStartKbps(40000, Double.POSITIVE_INFINITY));
        assertEquals(2000, NvHTTP.probeStartKbps(40000, 1));
        assertEquals(2000, NvHTTP.probeStartKbps(40000, 0));
        assertEquals(2000, NvHTTP.probeStartKbps(40000, Double.NaN));
        assertEquals(1500, NvHTTP.probeStartKbps(1500, 1));
    }

    @Test
    public void aLinkTestBodyMustBeBinaryNonEmptyAndNoLargerThanAsked() throws Exception {
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> {
            assertEquals("/bandwidth-probe", req.path);
            return reply.get();
        })) {
            NvHTTP http = server.newClient();
            reply.set(TestHttpServer.Response.bytes(200, "application/octet-stream", new byte[1000]));
            assertTrue(http.probeLinkMbps() > 0);
            reply.set(TestHttpServer.Response.bytes(200, "application/octet-stream", new byte[0]));
            assertThrows(IOException.class, http::probeLinkMbps);
            reply.set(TestHttpServer.Response.bytes(200, "application/octet-stream", new byte[NvHTTP.LINK_PROBE_BYTES + 1]));
            assertThrows(IOException.class, http::probeLinkMbps);
            reply.set(TestHttpServer.Response.bytes(200, "text/html", new byte[1000]));
            assertThrows(IOException.class, http::probeLinkMbps);
        }
    }
}
