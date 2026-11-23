package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nvstream.ConnectionContext;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static com.limelight.nvstream.http.LaunchConfirmation.State.*;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class LaunchConfirmationTest {
    @Test
    public void onlyHostConfirmationMessagesAllowASecondLaunch() throws Exception {
        NvApp game = new NvApp("Game", 73, false);
        NvApp terminate = new NvApp("Terminate", 2147483504, false);
        assertTrue(LaunchConfirmation.isRequired(game, NvHTTPRoleTest.error("replace-confirm")));
        assertTrue(LaunchConfirmation.isRequired(terminate, NvHTTPRoleTest.error("terminate-confirm")));
        assertFalse(LaunchConfirmation.isRequired(game, NvHTTPRoleTest.error("terminate-confirm")));
        assertFalse(LaunchConfirmation.isRequired(terminate, NvHTTPRoleTest.error("replace-confirm")));
        for (int code : new int[] {401, 403, 409, 410, 503}) {
            assertFalse(LaunchConfirmation.isRequired(game, new HostHttpResponseException(code, "Gone")));
        }
        assertFalse(LaunchConfirmation.isRequired(game, NvHTTPRoleTest.error("role-conflict")));
    }

    @Test
    public void completed410ActionsAreNotConfirmationRequestsOrStreamFailures() throws Exception {
        String[] replies = {"monitor-disconnected", "input-disconnected", "terminate-done"};
        for (int i = 0; i < replies.length; i++) {
            NvApp app = new NvApp("Action", 2147483502 + i, false);
            HostHttpResponseException error = NvHTTPRoleTest.error(replies[i]);
            assertTrue(LaunchConfirmation.isCompleted(app, error));
            assertFalse(LaunchConfirmation.isRequired(app, error));
            assertFalse(LaunchConfirmation.isCompleted(new NvApp("Game", 73, false), error));
        }
    }

    private static HostHttpResponseException reply(String message, boolean gameSession) throws Exception {
        String xml = NvHTTPRoleTest.completion(message, gameSession);
        try (TestHttpServer server = new TestHttpServer(req -> TestHttpServer.Response.xml(200, xml))) {
            NvHTTP http = server.newClient();
            ConnectionContext ctx = new ConnectionContext();
            ctx.streamConfig = new com.limelight.nvstream.StreamConfiguration.Builder().build();
            ctx.negotiatedWidth = 1920;
            ctx.negotiatedHeight = 1080;
            ctx.riKey = new javax.crypto.spec.SecretKeySpec(new byte[16], "AES");
            ctx.hostCaps = HostCapabilities.UNKNOWN;
            return assertThrows(HostHttpResponseException.class,
                    () -> http.launchApp(ctx, "launch", 42, false));
        }
    }

    @Test
    public void vibepolloAndApolloCompletionsFinishTheirActions() throws Exception {
        NvApp disconnectMonitor = new NvApp("Disconnect Monitor", 2147483502, false);
        NvApp disconnectInput = new NvApp("Disconnect Input", 2147483503, false);
        NvApp terminateTile = new NvApp("Terminate", 2147483504, false);
        NvApp secondaryTerminateTile = new NvApp(" Terminate", 2147483514, false);
        NvApp terminateApp = new NvApp("Terminate", 1219834627, false);
        terminateApp.setAppUuid("E16CBE1B-295D-4632-9A76-EC4180C857D3");
        terminateApp.setHostCapabilities(TestHosts.fixture("apollo-serverinfo.xml"));

        // Vibepollo remote_session.cpp successful_control_completion().
        HostHttpResponseException monitorDone = reply("Remote Monitor disconnected successfully.", true);
        HostHttpResponseException inputDone = reply("Remote Input disconnected successfully.", true);
        HostHttpResponseException streamDone = reply(
                "Active stream terminated. Remote Monitor and Remote Input remain connected.", true);
        // Apollo nvhttp.cpp launch() for its Terminate app; Vibepollo answers that app the same way.
        HostHttpResponseException appDone = reply("App terminated.", false);

        assertTrue(LaunchConfirmation.isCompleted(disconnectMonitor, monitorDone));
        assertTrue(LaunchConfirmation.isCompleted(disconnectInput, inputDone));
        for (NvApp terminate : new NvApp[] {terminateTile, secondaryTerminateTile}) {
            assertTrue(LaunchConfirmation.isCompleted(terminate, streamDone));
            assertFalse(LaunchConfirmation.isRequired(terminate, streamDone));
        }
        assertTrue(LaunchConfirmation.isCompleted(terminateApp, appDone));
        assertFalse(LaunchConfirmation.isRequired(terminateApp, appDone));

        // Each wording completes only its own action, and only as a 410.
        NvApp game = new NvApp("Game", 73, false);
        for (HostHttpResponseException done : new HostHttpResponseException[] {monitorDone, inputDone, streamDone, appDone}) {
            assertFalse(LaunchConfirmation.isCompleted(game, done));
        }
        assertFalse(LaunchConfirmation.isCompleted(disconnectMonitor, inputDone));
        assertFalse(LaunchConfirmation.isCompleted(disconnectInput, monitorDone));
        assertFalse(LaunchConfirmation.isCompleted(disconnectMonitor, streamDone));
        assertFalse(LaunchConfirmation.isCompleted(terminateTile, monitorDone));
        assertFalse(LaunchConfirmation.isCompleted(terminateTile, new HostHttpResponseException(409, "App terminated.")));
        assertFalse(LaunchConfirmation.isCompleted(terminateTile, new HostHttpResponseException(410, null)));
        for (String other : new String[] {"Terminated", "App not terminated.", "App terminated. Retry",
                "Remote session action conflicts with this client's current session state"}) {
            assertFalse(other, LaunchConfirmation.isCompleted(terminateTile, new HostHttpResponseException(410, other)));
        }
        // Case and the closing period may drift between versions; the words may not.
        assertTrue(LaunchConfirmation.isCompleted(terminateTile, new HostHttpResponseException(410, "application terminated.")));
        assertTrue(LaunchConfirmation.isCompleted(disconnectInput, new HostHttpResponseException(410, " Remote input disconnected ")));
    }

    @Test
    public void confirmationNeedsAnExplicitDecisionBeforeSixtySeconds() {
        LaunchConfirmation request = new LaunchConfirmation(false, 100);
        assertEquals(WAITING, request.getState(60_099));
        request.confirm(60_099);
        assertEquals(CONFIRMED, request.getState(60_100));
        request.cancel();
        assertEquals(CONFIRMED, request.getState(70_000));
    }

    @Test
    public void deadlineMatchesHostExpiryAndCannotBeConfirmedLater() throws Exception {
        LaunchConfirmation request = new LaunchConfirmation(true, 100);
        assertEquals(TIMED_OUT, request.getState(60_100));
        request.confirm(60_100);
        request.cancel();
        assertEquals(TIMED_OUT, request.await());
    }

    @Test
    public void cancellationIsTerminalAndUnblocksTheWaiter() throws Exception {
        LaunchConfirmation request = new LaunchConfirmation(false, LaunchConfirmation.nowMs());
        java.util.concurrent.ExecutorService worker = java.util.concurrent.Executors.newSingleThreadExecutor();
        try {
            java.util.concurrent.Future<LaunchConfirmation.State> result = worker.submit(request::await);
            request.cancel();
            assertEquals(CANCELLED, result.get(2, java.util.concurrent.TimeUnit.SECONDS));
            request.confirm(LaunchConfirmation.nowMs());
            assertEquals(CANCELLED, request.getState(LaunchConfirmation.nowMs()));
        } finally {
            worker.shutdownNow();
        }
    }
}
