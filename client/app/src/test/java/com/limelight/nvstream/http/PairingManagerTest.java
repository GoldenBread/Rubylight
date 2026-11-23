package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.BeforeClass;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.Signature;
import java.security.cert.X509Certificate;
import java.util.ArrayDeque;
import java.util.Arrays;
import java.util.Base64;
import java.util.HexFormat;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PairingManagerTest {
    private static final String SERVER_INFO = "<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>0</PairStatus><appversion>7.1.2.3</appversion></root>";
    private static String hostCertPemHex;

    @BeforeClass
    public static void createIdentity() throws Exception {
        TestHttpServer.ensureIdentities();
        String b64 = Base64.getMimeEncoder(64, "\n".getBytes(StandardCharsets.US_ASCII))
                .encodeToString(TestHttpServer.hostCert().getEncoded());
        String pem = "-----BEGIN CERTIFICATE-----\n" + b64 + "\n-----END CERTIFICATE-----\n";
        hostCertPemHex = HexFormat.of().formatHex(pem.getBytes(StandardCharsets.US_ASCII));
    }

    private static String reply(String tag, String value) {
        return "<root status_code=\"200\"><paired>1</paired><" + tag + ">" + value + "</" + tag + "></root>";
    }

    private static TestHttpServer scriptedServer(AtomicInteger cancellations, boolean failCancel, String... replies)
            throws IOException {
        ArrayDeque<String> queue = new ArrayDeque<>(Arrays.asList(replies));
        return new TestHttpServer(req -> {
            if ("/unpair".equals(req.path)) {
                cancellations.incrementAndGet();
                if (failCancel) return TestHttpServer.Response.text(500, "cancel failed");
                return TestHttpServer.Response.xml(200, "<root status_code=\"200\"><paired>0</paired></root>");
            }
            if (queue.isEmpty()) {
                return TestHttpServer.Response.text(500, "unexpected extra request: " + req.rawQuery);
            }
            return TestHttpServer.Response.xml(200, queue.removeFirst());
        });
    }

    private static PairingLink link(X509Certificate host) throws Exception {
        return PairingLink.parse("rubylight://pair?v=1&host=127.0.0.1&port=47989&otp=1234&pass=abcd1234&fp="
                + TestHttpServer.sha256Hex(host));
    }

    @Test
    public void aLinkNamingAnotherCertificateIsRefusedBeforeItIsTrusted() throws Exception {
        AtomicInteger cancellations = new AtomicInteger();
        try (TestHttpServer server = scriptedServer(cancellations, false, reply("plaincert", hostCertPemHex))) {
            NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
            PairingManager pairing = http.getPairingManager();
            assertEquals(PairingManager.PairState.CERT_MISMATCH,
                    pairing.pair(SERVER_INFO, link(TestHttpServer.otherCert())));
            assertNull(pairing.getPairedCert());
            assertEquals(1, cancellations.get());
            for (TestHttpServer.RecordedRequest req : server.requests()) {
                assertNull(req.queryParam("clientchallenge"));
            }
        }
    }

    @Test
    public void aReplayedLinkIsAHostRefusalNotANetworkError() throws Exception {
        AtomicInteger cancellations = new AtomicInteger();
        String refusedXml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"400\""
                + " status_message=\"no one-time pairing code is live: it was used or has expired; show a new one"
                + " on the PC\"><paired>0</paired></root>";
        try (TestHttpServer server = scriptedServer(cancellations, false, refusedXml)) {
            NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
            HostHttpResponseException refusal = assertThrows(HostHttpResponseException.class,
                    () -> http.getPairingManager().pair(SERVER_INFO, link(TestHttpServer.hostCert())));
            assertEquals(400, refusal.getErrorCode());
            assertTrue(refusal.getMessage(), refusal.getMessage().contains("show a new one"));
        }
    }

    @Test
    public void aLinkNamingTheHostCertificateGoesOnToTheChallenge() throws Exception {
        AtomicInteger cancellations = new AtomicInteger();
        try (TestHttpServer server = scriptedServer(cancellations, false,
                reply("plaincert", hostCertPemHex), reply("challengeresponse", "AA"))) {
            NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
            assertThrows(IOException.class,
                    () -> http.getPairingManager().pair(SERVER_INFO, link(TestHttpServer.hostCert())));
            assertEquals(1, cancellations.get());
        }
    }

    @Test
    public void malformedCertificatesFailWithCheckedErrorsAndReleasePairing() throws Exception {
        for (String encoded : new String[] {"0", "GG", "00"}) {
            AtomicInteger cancellations = new AtomicInteger();
            try (TestHttpServer server = scriptedServer(cancellations, false, reply("plaincert", encoded))) {
                NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
                assertThrows(IOException.class, () -> http.getPairingManager().pair(SERVER_INFO, "1234"));
                assertEquals(1, cancellations.get());
            }
        }
    }

    @Test
    public void truncatedEncryptedChallengesFailBeforeTheNextProtocolStep() throws Exception {
        for (String encoded : new String[] {"AA", "00".repeat(16), "00".repeat(47)}) {
            AtomicInteger cancellations = new AtomicInteger();
            try (TestHttpServer server = scriptedServer(cancellations, false,
                    reply("plaincert", hostCertPemHex), reply("challengeresponse", encoded))) {
                NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
                assertThrows(IOException.class, () -> http.getPairingManager().pair(SERVER_INFO, "1234"));
                assertEquals(1, cancellations.get());
            }
        }
    }

    @Test
    public void truncatedSecretsAreCheckedErrors() throws Exception {
        AtomicInteger cancellations = new AtomicInteger();
        try (TestHttpServer server = scriptedServer(cancellations, false,
                reply("plaincert", hostCertPemHex), reply("challengeresponse", "00".repeat(48)),
                reply("pairingsecret", "AA"))) {
            NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
            assertThrows(IOException.class, () -> http.getPairingManager().pair(SERVER_INFO, "1234"));
            assertEquals(1, cancellations.get());
        }
    }

    @Test
    public void invalidSignatureFailsPairingWithoutCrashing() throws Exception {
        AtomicInteger cancellations = new AtomicInteger();
        try (TestHttpServer server = scriptedServer(cancellations, false,
                reply("plaincert", hostCertPemHex), reply("challengeresponse", "00".repeat(48)),
                reply("pairingsecret", "00".repeat(17)))) {
            NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
            assertEquals(PairingManager.PairState.SERVER_UNVERIFIED,
                    http.getPairingManager().pair(SERVER_INFO, "1234"));
            assertEquals(1, cancellations.get());
        }
    }

    @Test
    public void cleanupFailureDoesNotHideWrongPin() throws Exception {
        byte[] secret = new byte[16];
        Signature signature = Signature.getInstance("SHA256withRSA");
        signature.initSign(TestHttpServer.hostKey());
        signature.update(secret);
        AtomicInteger cancellations = new AtomicInteger();
        try (TestHttpServer server = scriptedServer(cancellations, true,
                reply("plaincert", hostCertPemHex), reply("challengeresponse", "00".repeat(48)),
                reply("pairingsecret", HexFormat.of().formatHex(secret) + HexFormat.of().formatHex(signature.sign())))) {
            NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
            assertEquals(PairingManager.PairState.PIN_WRONG,
                    http.getPairingManager().pair(SERVER_INFO, "1234"));
            assertEquals(1, cancellations.get());
        }
    }

    @Test
    public void pairedCertificateCannotBeReplacedByAnotherCertificate() throws Exception {
        try (TestHttpServer wrongHost = new TestHttpServer(req -> TestHttpServer.Response.json(
                200, "{\"version\":1,\"sampled_at_unix_ms\":1791712800000,\"interval_ms\":1000,\"cpu\":{},\"gpus\":[]}"))) {
            NvHTTP http = new NvHTTP(
                    new ComputerDetails.AddressTuple("127.0.0.1", wrongHost.httpPort()),
                    wrongHost.httpsPort(),
                    "client",
                    TestHttpServer.otherCert(),
                    NativeApiFixtures.crypto());
            assertThrows(IOException.class, http::getHardwareMetrics);
            assertEquals(0, wrongHost.requests().size());
        }
    }

    @Test
    public void pairedLeafRemainsTrustedForHttpsHostCalls() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(
                200, "<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus></root>"))) {
            NvHTTP http = server.newClient(true);
            String reply = http.getServerInfo(true);
            assertTrue(reply.contains("<uniqueid>host-1</uniqueid>"));
            assertFalse(http.serverInfoFromAnotherCertificate());
            assertEquals("https", server.requests().get(0).scheme);
        }
    }

    @Test
    public void unpairedHostsCannotMakeAuthenticatedHttpsCalls() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(
                200, "<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>1</PairStatus></root>"))) {
            NvHTTP http = server.newClient(false);
            assertTrue(http.getServerInfo(true).contains("host-1"));
            assertThrows(IOException.class, http::getHardwareMetrics);
        }
    }
}
