package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.preferences.HostDisplayChoice;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.InputStream;
import java.io.StringReader;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;

import javax.crypto.spec.SecretKeySpec;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPRoleTest {
    private static final class Query {
        private final Map<String, String> params = new LinkedHashMap<>();

        Query(String rawQuery) {
            for (String pair : rawQuery.split("&")) {
                if (pair.isEmpty()) continue;
                int eq = pair.indexOf('=');
                String key = URLDecoder.decode(eq < 0 ? pair : pair.substring(0, eq), StandardCharsets.UTF_8);
                String val = eq < 0 ? "" : URLDecoder.decode(pair.substring(eq + 1), StandardCharsets.UTF_8);
                params.putIfAbsent(key, val);
            }
        }

        String queryParameter(String name) {
            return params.get(name);
        }
    }

    public static String fixture(String name) throws Exception {
        try (InputStream input = NvHTTPRoleTest.class.getResourceAsStream("/protocol/butterpollo-" + name + ".xml")) {
            assertNotNull(input);
            return new String(input.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    public static HostHttpResponseException error(String name) throws Exception {
        String xml = fixture(name);
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(200, xml))) {
            NvHTTP http = server.newClient();
            ConnectionContext ctx = new ConnectionContext();
            ctx.streamConfig = new StreamConfiguration.Builder().build();
            ctx.negotiatedWidth = 1920;
            ctx.negotiatedHeight = 1080;
            ctx.riKey = new SecretKeySpec(new byte[16], "AES");
            ctx.hostCaps = HostCapabilities.UNKNOWN;
            return assertThrows(HostHttpResponseException.class,
                    () -> http.launchApp(ctx, "launch", 42, false));
        }
    }

    private List<NvApp> apps(String catalogue) throws Exception {
        return NvHTTP.getAppListByReader(new StringReader(fixture(catalogue)));
    }

    @Test
    public void hostPrimaryCatalogueHasBothRemoteRoles() throws Exception {
        List<NvApp> apps = apps("remote-applist");
        assertEquals(3, apps.size());
        assertEquals(NvApp.Role.STREAM, apps.get(0).getRole());
        assertEquals(NvApp.Role.INPUT_ONLY, apps.get(1).getRole());
        assertEquals(NvApp.Role.REMOTE_MONITOR, apps.get(2).getRole());
    }

    @Test
    public void monitorResumeIsScopedToTheMonitorCatalogue() throws Exception {
        List<NvApp> monitor = apps("monitor-applist");
        assertEquals(NvApp.Control.RESUME, monitor.get(0).getControl());
        assertEquals(NvApp.Role.REMOTE_MONITOR, monitor.get(0).getRole());
        assertTrue(monitor.get(1).isControlAction());
        List<NvApp> secondary = apps("secondary-applist");
        assertEquals(NvApp.Control.RUNNING_GAME, secondary.get(0).getControl());
        assertEquals(NvApp.Control.RESUME, secondary.get(1).getControl());
        assertEquals(NvApp.Role.STREAM, secondary.get(1).getRole());
        assertEquals(NvApp.Control.TERMINATE, secondary.get(2).getControl());
        assertEquals(NvApp.Role.INPUT_ONLY, secondary.get(4).getRole());
        assertEquals(NvApp.Role.REMOTE_MONITOR, secondary.get(5).getRole());
    }

    @Test
    public void hostIdentityAliasesSurviveRenamesAndShortcutIdChanges() {
        for (int offset = 1; offset <= 7; offset++) {
            NvApp.Control expected = NvApp.Control.values()[offset];
            for (int base : new int[] {2147483500, 2147483600}) {
                assertEquals(expected, new NvApp("Renamed", base + offset, false).getControl());
            }
            if (offset == 1 || offset == 4 || offset == 5 || offset == 6) {
                assertEquals(expected, new NvApp("Renamed", 2147483510 + offset, false).getControl());
            }
            NvApp shortcut = new NvApp("Old name", 0, false);
            shortcut.setAppUuid("9a1c5a25-58fe-40e0-b9aa-7d3f0000000" + offset);
            assertEquals(expected, shortcut.getControl());
        }
    }

    private static final String APOLLO_APPLIST = "<root status_code=\"200\">" +
            apolloApp("Game", "0F3C2B1A-1111-4222-8333-944455556666", 0, 1385710102) +
            apolloApp("Virtual Display", "8902CB19-674A-403D-A587-41B092E900BA", 1, 1913204568) +
            apolloApp("Desktop (fallback)", "EAAC6159-089A-46A9-9E24-6436885F6610", 2, 1093255296) +
            apolloApp("Remote Input", "8CB5C136-DA67-4F99-B4A1-F9CD35005CF4", 3, 1557224430) +
            apolloApp("Terminate", "E16CBE1B-295D-4632-9A76-EC4180C857D3", 4, 1219834627) + "</root>";

    private static String apolloApp(String title, String uuid, int index, int id) {
        return "<App><IsHdrSupported>0</IsHdrSupported><AppTitle>" + title + "</AppTitle><UUID>" + uuid +
                "</UUID><IDX>" + index + "</IDX><ID>" + id + "</ID></App>";
    }

    @Test
    public void apolloRemoteInputAndTerminateAreRolesOnlyOnApolloAndVibepollo() throws Exception {
        for (HostCapabilities caps : new HostCapabilities[] {TestHosts.fixture("apollo-serverinfo.xml"),
                TestHosts.fixture("apollo-http-serverinfo.xml"), TestHosts.fixture("vibepollo-serverinfo.xml"),
                HostCapabilities.UNKNOWN}) {
            List<NvApp> apps = NvHTTP.getAppListByReader(new StringReader(APOLLO_APPLIST), caps);
            for (int i : new int[] {0, 1, 2}) {
                assertEquals(caps + " " + i, NvApp.Control.NONE, apps.get(i).getControl());
                assertEquals(caps + " " + i, NvApp.Role.STREAM, apps.get(i).getRole());
            }
            assertEquals(caps.toString(), NvApp.Control.INPUT, apps.get(3).getControl());
            assertEquals(caps.toString(), NvApp.Role.INPUT_ONLY, apps.get(3).getRole());
            assertFalse(caps.toString(), apps.get(3).isControlAction());
            assertEquals(caps.toString(), NvApp.Control.TERMINATE, apps.get(4).getControl());
            assertTrue(caps.toString(), apps.get(4).isControlAction());
        }
        for (String host : new String[] {"sunshine", "gamestream", "rubylight"}) {
            for (NvApp app : NvHTTP.getAppListByReader(new StringReader(APOLLO_APPLIST),
                    TestHosts.fixture(host + "-serverinfo.xml"))) {
                assertEquals(host, NvApp.Control.NONE, app.getControl());
                assertEquals(host, NvApp.Role.STREAM, app.getRole());
            }
        }
        NvApp shortcut = new NvApp("Remote Input", 0, false);
        shortcut.setAppUuid("8cb5c136-da67-4f99-b4a1-f9cd35005cf4");
        shortcut.setHostCapabilities(TestHosts.fixture("apollo-serverinfo.xml"));
        assertEquals(NvApp.Role.INPUT_ONLY, shortcut.getRole());
    }

    @Test
    public void apolloRemoteInputLaunchesInputOnlyByItsUuid() throws Exception {
        NvApp app = NvHTTP.getAppListByReader(new StringReader(APOLLO_APPLIST),
                TestHosts.fixture("apollo-serverinfo.xml")).get(3);
        ConnectionContext context = context(app);
        context.hostCaps = TestHosts.fixture("apollo-serverinfo.xml");
        Query query = query(context, "launch");
        assertEquals("1557224430", query.queryParameter("appid"));
        assertEquals("8CB5C136-DA67-4F99-B4A1-F9CD35005CF4", query.queryParameter("appuuid"));
        assertEquals("1", query.queryParameter("input_only"));
        assertNull(query.queryParameter("hdrMode"));
        assertNull(query.queryParameter("virtualDisplay"));
    }

    @Test
    public void microphoneOnlyLaunchesInputOnlyWithoutAnApp() throws Exception {
        NvApp app = NvApp.micOnly("Microphone only");
        assertEquals(NvApp.Role.INPUT_ONLY, app.getRole());
        assertFalse(app.isSmallStreamInputOnly());
        assertFalse(app.isControlAction());
        Query query = query(context(app), "launch");
        assertEquals("0", query.queryParameter("appid"));
        assertEquals("1", query.queryParameter("input_only"));
        assertEquals("1", query.queryParameter("rlMicOnly"));
        assertNull(query.queryParameter("virtualDisplay"));
        assertNull(query.queryParameter("hdrMode"));
        assertNull(query(context(apps("remote-applist").get(1)), "launch").queryParameter("rlMicOnly"));
    }

    @Test
    public void ordinaryNamesAndUnknownXmlFieldsDoNotAssignARole() throws Exception {
        NvApp app = NvHTTP.getAppListByReader(new StringReader(
                "<root status_code=\"200\"><App><ID>42</ID><AppTitle>Remote Monitor</AppTitle>" +
                        "<UUID>unknown</UUID><role>input_only</role></App></root>")).getFirst();
        assertEquals(NvApp.Role.STREAM, app.getRole());
        assertEquals(NvApp.Control.NONE, app.getControl());
    }

    private ConnectionContext context(NvApp app) {
        ConnectionContext context = new ConnectionContext();
        context.streamConfig = new StreamConfiguration.Builder().setApp(app)
                .setLaunchRefreshRate(60).setAttachedGamepadMask(3)
                .setPersistGamepadsAfterDisconnect(true).setVirtualDisplay(true, 150).setVrr(true).build();
        context.hostCaps = TestHosts.rubylight();
        context.negotiatedWidth = 1920;
        context.negotiatedHeight = 1080;
        context.riKey = new SecretKeySpec(new byte[16], "AES");
        context.riKeyId = -123;
        return context;
    }

    private Query query(ConnectionContext context, String verb) {
        return new Query(NvHTTP.getLaunchQuery(context, context.streamConfig.getApp().getAppId(), true));
    }

    @Test
    public void monitorLaunchAndResumeRequestVideoWithoutControllers() throws Exception {
        for (NvApp app : new NvApp[] {apps("remote-applist").get(2), apps("monitor-applist").get(0)}) {
            for (String verb : new String[] {"launch", "resume"}) {
                Query query = query(context(app), verb);
                assertEquals("1", query.queryParameter("remote_monitor"));
                assertNull(query.queryParameter("input_only"));
                assertEquals("0", query.queryParameter("gcmap"));
                assertEquals("0", query.queryParameter("remoteControllersBitmap"));
                assertEquals("0", query.queryParameter("gcpersist"));
                assertEquals("1", query.queryParameter("hdrMode"));
                assertEquals("1", query.queryParameter("virtualDisplay"));
                assertEquals("1", query.queryParameter("clientVrrRequested"));
                assertEquals("1", query.queryParameter("vrr"));
                assertEquals(app.getAppUuid(), query.queryParameter("appuuid"));
            }
        }
    }

    @Test
    public void onlyAStreamSendsTheDisplayChoice() throws Exception {
        List<NvApp> remote = apps("remote-applist");
        for (NvApp app : new NvApp[] {remote.get(2), remote.get(1)}) {
            ConnectionContext context = context(app);
            context.hostDisplay = HostDisplayChoice.PHYSICAL;
            for (String verb : new String[] {"launch", "resume"}) {
                assertNull(query(context, verb).queryParameter("hostDisplay"));
            }
        }
        ConnectionContext context = context(remote.get(0));
        context.hostDisplay = HostDisplayChoice.EXTENDED;
        for (String verb : new String[] {"launch", "resume"}) {
            Query query = query(context, verb);
            assertEquals("extended", query.queryParameter("hostDisplay"));
            assertEquals("1", query.queryParameter("virtualDisplay"));
        }
        context.hostCaps = TestHosts.fixture("gamestream-serverinfo.xml");
        assertNull(query(context, "launch").queryParameter("hostDisplay"));
    }

    @Test
    public void inputOnlyRequestsInputWithoutDisplayHdrOrVrrChanges() throws Exception {
        ConnectionContext context = context(apps("remote-applist").get(1));
        for (String verb : new String[] {"launch", "resume"}) {
            Query query = query(context, verb);
            assertEquals("1", query.queryParameter("input_only"));
            assertNull(query.queryParameter("remote_monitor"));
            assertNull(query.queryParameter("hdrMode"));
            assertNull(query.queryParameter("virtualDisplay"));
            assertNull(query.queryParameter("scaleFactor"));
            assertNull(query.queryParameter("clientVrrRequested"));
            assertNull(query.queryParameter("vrr"));
            assertEquals("3", query.queryParameter("gcmap"));
            assertEquals("-123", query.queryParameter("rikeyid"));
        }
    }

    @Test
    public void smallStreamInputOnlyIsAnOrdinarySmallStreamToTheHost() {
        NvApp app = new NvApp("Game", 73, true);
        app.setSmallStreamInputOnly(true);
        for (String host : new String[] {"sunshine-serverinfo.xml", "gamestream-serverinfo.xml", "apollo-serverinfo.xml"}) {
            app.setHostCapabilities(TestHosts.fixture(host));
            assertEquals(host, NvApp.Role.INPUT_ONLY, app.getRole());
            ConnectionContext context = context(app);
            context.hostCaps = TestHosts.fixture(host);
            context.negotiatedWidth = 640;
            context.negotiatedHeight = 360;
            for (String verb : new String[] {"launch", "resume"}) {
                Query query = query(context, verb);
                assertNull(host, query.queryParameter("input_only"));
                assertNull(host, query.queryParameter("remote_monitor"));
                assertNull(host, query.queryParameter("hdrMode"));
                assertNull(host, query.queryParameter("virtualDisplay"));
                assertNull(host, query.queryParameter("clientVrrRequested"));
                assertEquals(host, "640x360x60", query.queryParameter("mode"));
                assertEquals(host, "3", query.queryParameter("gcmap"));
            }
        }
    }

    @Test
    public void controlTilesIgnoreTheSmallStream() throws Exception {
        NvApp tile = apps("remote-applist").get(1);
        assertEquals(NvApp.Role.INPUT_ONLY, tile.getRole());
        tile.setSmallStreamInputOnly(true);
        assertFalse(tile.isSmallStreamInputOnly());
        assertEquals("1", query(context(tile), "launch").queryParameter("input_only"));

        NvApp monitor = apps("remote-applist").get(2);
        monitor.setSmallStreamInputOnly(true);
        assertEquals(NvApp.Role.REMOTE_MONITOR, monitor.getRole());

        NvApp game = new NvApp("Game", 73, true);
        assertFalse(game.isSmallStreamInputOnly());
        assertEquals(NvApp.Role.STREAM, game.getRole());
    }

    @Test
    public void ordinaryLaunchKeepsItsDisplayAndControllerParameters() {
        Query query = query(context(new NvApp("Game", 73, true)), "launch");
        assertNull(query.queryParameter("remote_monitor"));
        assertNull(query.queryParameter("input_only"));
        assertEquals("3", query.queryParameter("gcmap"));
        assertEquals("1", query.queryParameter("gcpersist"));
        assertEquals("1", query.queryParameter("hdrMode"));
        assertEquals("150", query.queryParameter("scaleFactor"));
        assertEquals("1", query.queryParameter("clientVrrRequested"));
        assertEquals("150", query.queryParameter("scaleFactor"));
        assertEquals("1", query.queryParameter("clientVrrRequested"));
        assertEquals("1", query.queryParameter("vrr"));
    }

    @Test
    public void uuidOnlyShortcutStillRequestsTheRemoteRole() {
        NvApp app = new NvApp("Remote Input", 0, false);
        app.setAppUuid("9a1c5a25-58fe-40e0-b9aa-7d3f00000006");
        Query query = query(context(app), "launch");
        assertEquals("0", query.queryParameter("appid"));
        assertEquals(app.getAppUuid(), query.queryParameter("appuuid"));
        assertEquals("1", query.queryParameter("input_only"));
        assertNull(query.queryParameter("remote_monitor"));
    }

    @Test
    public void roleSuccessUsesTheRequestedEndpointsReplyAndEncryptedRtspUrl() throws Exception {
        for (String verb : new String[] {"launch", "resume"}) {
            String xml = fixture("role-" + verb);
            try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(200, xml))) {
                ConnectionContext context = context(new NvApp("Game", 42, false));
                assertTrue(server.newClient().launchApp(context, verb, 42, false));
                assertEquals("rtspenc://192.0.2.1:48010", context.rtspSessionUrl);
            }
        }
    }

    @Test
    public void disconnectingEitherRoleUsesItsControlTileAndNeverCancel() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        java.util.concurrent.atomic.AtomicReference<String> nextXml =
                new java.util.concurrent.atomic.AtomicReference<>();
        java.util.concurrent.atomic.AtomicBoolean expectMonitor =
                new java.util.concurrent.atomic.AtomicBoolean();
        try (TestHttpServer server = new TestHttpServer(req -> {
            requests.incrementAndGet();
            assertEquals("/launch", req.path);
            assertEquals(expectMonitor.get() ? "2147483502" : "2147483503", req.queryParam("appid"));
            assertTrue(req.queryParam("rikey").matches("[0-9A-F]{32}"));
            assertEquals("0", req.queryParam("rikeyid"));
            assertEquals("0", req.queryParam("localAudioPlayMode"));
            assertNull(req.queryParam("appuuid"));
            return TestHttpServer.Response.xml(200, nextXml.get());
        })) {
            NvHTTP http = server.newClient();
            for (NvApp.Role role : new NvApp.Role[] {NvApp.Role.REMOTE_MONITOR, NvApp.Role.INPUT_ONLY}) {
                boolean monitor = role == NvApp.Role.REMOTE_MONITOR;
                expectMonitor.set(monitor);
                for (String xml : new String[] {fixture(monitor ? "monitor-disconnected" : "input-disconnected"),
                        completion(monitor ? "Remote Monitor disconnected successfully." :
                                "Remote Input disconnected successfully.", true)}) {
                    nextXml.set(xml);
                    assertTrue(xml, http.disconnectRole(role));
                }
            }
            assertThrows(IllegalArgumentException.class, () -> http.disconnectRole(NvApp.Role.STREAM));
            assertEquals(4, requests.get());
        }
    }

    /**
     * A 410 completion as Vibepollo writes it for its control tiles (nvhttp.cpp launch(),
     * remote_session.cpp successful_control_completion()), or as Apollo and Vibepollo write it for
     * Apollo's Terminate app, without the gamesession element.
     */
    public static String completion(String message, boolean gameSession) {
        return "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"410\" status_message=\"" + message +
                "\"><resume>0</resume>" + (gameSession ? "<gamesession>0</gamesession>" : "") + "</root>";
    }
}
