package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowBuild;

import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class)
public class NvHTTPPairingTest {
    @Test
    public void pairingRequestsPreserveDeviceNamesWithoutAddingQueryParameters() throws Exception {
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(
                200, "<root status_code=\"200\"><paired>0</paired></root>"))) {
            for (String model : new String[] {"Odin2 Portal", "SM-F956B", "München's handheld",
                    "Model &uniqueid=other+50%#"}) {
                ShadowBuild.setModel(model);
                NvHTTP http = server.newClient("client", false, NativeApiFixtures.crypto());
                int before = server.requests().size();
                PairingManager pm = http.getPairingManager();
                assertEquals(PairingManager.PairState.FAILED, pm.pair("<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>0</PairStatus><appversion>7.1.2.3</appversion></root>", "0042"));
                assertEquals(PairingManager.PairState.FAILED, pm.pair("<root status_code=\"200\"><uniqueid>host-1</uniqueid><state>SUNSHINE_SERVER_FREE</state><PairStatus>0</PairStatus><appversion>7.1.2.3</appversion></root>", "0042", "secret"));

                List<TestHttpServer.RecordedRequest> reqs = server.requests().subList(before, server.requests().size());
                assertEquals(2, reqs.size());
                for (TestHttpServer.RecordedRequest req : reqs) {
                    assertEquals("http", req.scheme);
                    assertEquals("/pair", req.path);
                    assertEquals(List.of(model), req.queryValues("devicename"));
                    assertEquals(List.of("client"), req.queryValues("uniqueid"));
                    assertEquals("1", req.queryParam("updateState"));
                    assertNotNull(req.queryParam("uuid"));
                    assertEquals("getservercert", req.queryParam("phrase"));
                    assertNotNull(req.queryParam("salt"));
                    assertNotNull(req.queryParam("clientcert"));
                }
                assertNull(reqs.get(0).queryParam("otpauth"));
                assertTrue(reqs.get(1).queryParam("otpauth").matches("[0-9A-F]{64}"));
            }
        }
    }
}
