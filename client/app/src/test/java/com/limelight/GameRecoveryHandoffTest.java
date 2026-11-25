package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;

import com.limelight.GameRecoveryTestSupport.RecoveryBridge;
import com.limelight.GameRecoveryTestSupport.StartOrder;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.atomic.AtomicInteger;

import static com.limelight.GameRecoveryTestSupport.*;
import static org.junit.Assert.*;

/**
 * Tests {@link GameRecovery} split-screen task hand-off, network-loss/return route probing,
 * display-detach hand-offs, and recovery queue/teardown cancellation.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameRecoveryHandoffTest {
    // Retroid, Android 13, 2026-11-20: a shortcut's stream (alone in its task) in split screen went off with the screen.
    // Its return and then Reconnect each finished the stream before starting the next; Android closed split screen,
    // moved the new stream's task behind the home screen and removed it before it started. Such a stream now starts
    // the next one in its own task first. What Android then does with the task shows only on a device.
    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void inSplitScreenAStreamAloneInItsTaskStartsTheNextOneInThatTask() throws Exception {
        // {alone in its task, split screen, picture-in-picture, replaced in place}
        boolean[][] windows = {
                {true, true, false, true},
                {true, false, false, false},
                {false, true, false, false},
                {true, true, true, false},
        };
        for (boolean[] window : windows) {
            String condition = "root " + window[0] + ", split " + window[1] + ", pip " + window[2];
            Game game = establishedGame();
            placeWindow(game, window[0], window[1], window[2]);
            StartOrder order = new StartOrder(game);
            try {
                game.streamRecovery.reconnectStream();
                assertHandOff("Reconnect, " + condition, game, order, awaitStartedActivity(game), window[3]);
            } finally {
                order.close();
                cleanupRecovery(game);
            }
        }
        for (boolean split : new boolean[] {true, false}) {
            // The return after the screen was off (the user's own trip), as in that log.
            Game game = establishedGame();
            placeWindow(game, true, split, false);
            StartOrder order = new StartOrder(game);
            try {
                game.onPause();
                game.onStop();
                assertNotNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
                game.onResume();
                assertHandOff("return, split " + split, game, order, awaitRecoveryLaunch(game, 1), split);
            } finally {
                order.close();
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void inSplitScreenThePinScreenOfAReconnectGoesOnTopOfTheStreamInItsTask() throws Exception {
        com.limelight.pin.PinGate.resetForTests();
        com.limelight.pin.PinStore store = new com.limelight.pin.PinStore(RuntimeEnvironment.getApplication());
        store.setPin("2468".getBytes(java.nio.charset.StandardCharsets.US_ASCII));
        store.setMode(com.limelight.pin.PinStore.Mode.STREAMS);
        try {
            for (boolean split : new boolean[] {true, false}) {
                Game game = establishedGame();
                placeWindow(game, true, split, false);
                StartOrder order = new StartOrder(game);
                try {
                    game.streamRecovery.reconnectStream(true, false);
                    Intent started = awaitStartedActivity(game);
                    assertEquals(com.limelight.pin.PinLockActivity.class.getName(), started.getComponent().getClassName());
                    assertEquals("a plain start: in place it goes into this stream's task", 0,
                            started.getFlags() & (Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK));
                    assertEquals("split " + split, java.util.Collections.singletonList(!split), order.finishingAtStart);
                    Intent next = started.getParcelableExtra(com.limelight.pin.PinLockActivity.EXTRA_NEXT);
                    assertEquals(Game.class.getName(), next.getComponent().getClassName());
                    assertTrue(game.isFinishing());
                } finally {
                    order.close();
                    cleanupRecovery(game);
                }
            }
        } finally {
            store.setMode(com.limelight.pin.PinStore.Mode.OFF);
            com.limelight.pin.PinGate.resetForTests(); com.limelight.pin.PinStore.clearForTests(RuntimeEnvironment.getApplication());
        }
    }

    // A network back where the connection cannot carry on (another address, not roamed): the
    // overlay said a reconnect follows, and the recovery starts with no delay instead of after the
    // connection's timeout, also when that answer only comes once the network's address arrived.
    // It waits while the PC has no route (DHCP still running), starts once it does, and resumes
    // the captured game.
    @Test public void aNetworkBackTheConnectionCannotCarryOnOverReconnectsOnceThePcIsReachable() throws Exception {
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofSeconds(10));
        Game game = establishedGame();
        android.widget.TextView overlay = new android.widget.TextView(game);
        ReflectionHelpers.setField(game, "notificationOverlayView", overlay);
        AtomicInteger checks = new AtomicInteger();
        java.util.concurrent.atomic.AtomicBoolean routed = new java.util.concurrent.atomic.AtomicBoolean();
        Game.RouteProbe probe = (host, port) -> {
            checks.incrementAndGet();
            return routed.get() ? null : "ENETUNREACH (no address yet)";
        };
        ReflectionHelpers.setField(game.streamRecovery, "routeProbe", probe);
        game.setIntent(new Intent().putExtra(Game.EXTRA_HOST, "pc.test"));
        try {
            game.streamNetworkLost();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(game.getString(R.string.stream_network_lost_reconnecting), overlay.getText().toString());
            assertEquals(android.view.View.VISIBLE, overlay.getVisibility());
            // A poor-connection update meanwhile does not replace the notice.
            game.connectionStatusUpdate(MoonBridge.CONN_STATUS_OKAY);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(android.view.View.VISIBLE, overlay.getVisibility());

            // Back with no route yet: the connection carries on, and the notice goes.
            game.streamNetworkReturned(MoonBridge.NETWORK_CARRIES_ON, false);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(android.view.View.GONE, overlay.getVisibility());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            // Its address arrives, another one: the same return, reported again, reconnects.
            game.streamNetworkReturned(MoonBridge.NETWORK_CANNOT_CONTINUE, false);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(android.view.View.GONE, overlay.getVisibility());
            // No time passes on the main looper: the attempt does not wait for a backoff delay,
            // only for the route.
            long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MILLISECONDS.toNanos(HANG_GUARD_MS);
            while (checks.get() < 2) {
                if (System.nanoTime() > deadline) fail("the recovery never checked the route twice");
                Thread.sleep(5);
            }
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertNull("no attempt without a route", org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            routed.set(true);
            Intent retry = awaitStartedActivity(game);
            assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
            assertEquals("captured-game", retry.getStringExtra(Game.EXTRA_APP_UUID));
            assertEquals(1, retry.getIntExtra("RecoveryAttempts", 0));
            assertTrue(checks.get() >= 3);
        } finally {
            routed.set(true);
            cleanupRecovery(game);
        }
    }

    // The regression the review found: roamed is false, but the same address came back (DHCP
    // gave it again) or there is no route yet. The connection resumes in place, so nothing tears
    // it down; neither when it roamed, nor where a reconnect is not wanted.
    @Test public void aNetworkBackTheConnectionCarriesOnOverKeepsTheStream() throws Exception {
        Game game = establishedGame();
        android.widget.TextView overlay = new android.widget.TextView(game);
        ReflectionHelpers.setField(game, "notificationOverlayView", overlay);
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        try {
            for (int change : new int[] {MoonBridge.NETWORK_CARRIES_ON, MoonBridge.NETWORK_ROAMED}) {
                game.streamNetworkLost();
                game.streamNetworkReturned(change, false);
                org.robolectric.shadows.ShadowLooper.idleMainLooper();
                assertEquals(android.view.View.GONE, overlay.getVisibility());
                assertKeepsStreaming(game, "change " + change);
            }
            // A network drop tolerance holds the connection and decides.
            config.networkGraceSeconds = 20;
            game.streamNetworkLost();
            game.streamNetworkReturned(MoonBridge.NETWORK_CANNOT_CONTINUE, false);
            assertKeepsStreaming(game, "with a drop tolerance");
            // Mobile data without "Continue on mobile data": no reconnect over it.
            config.networkGraceSeconds = 0;
            config.roamingMetered = false;
            game.streamNetworkLost();
            game.streamNetworkReturned(MoonBridge.NETWORK_CANNOT_CONTINUE, true);
            assertKeepsStreaming(game, "on mobile data");
            // Automatic reconnection off: the user decides.
            config.autoReconnect = false;
            game.streamNetworkLost();
            game.streamNetworkReturned(MoonBridge.NETWORK_CANNOT_CONTINUE, false);
            assertKeepsStreaming(game, "with automatic reconnection off");
        } finally {
            cleanupRecovery(game);
        }
    }

    // The notice promises a reconnect only when one will follow.
    @Test public void theLostNetworkNoticePromisesAReconnectOnlyWhenOneFollows() throws Exception {
        Game game = establishedGame();
        android.widget.TextView overlay = new android.widget.TextView(game);
        ReflectionHelpers.setField(game, "notificationOverlayView", overlay);
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        String plain = game.getString(R.string.stream_network_lost);
        try {
            config.autoReconnect = false;
            assertEquals("automatic reconnection off", plain, noticeAfterLoss(game, overlay));
            config.autoReconnect = true;
            config.networkGraceSeconds = 30;
            assertEquals("a drop tolerance decides", plain, noticeAfterLoss(game, overlay));
            config.networkGraceSeconds = 0;
            setBudget(game, NightjarPolicyJni.recoveryMaxAttempts(), SystemClock.elapsedRealtime());
            assertEquals("the retry budget is used up", plain, noticeAfterLoss(game, overlay));
            setBudget(game, 0, 0);
            assertEquals(game.getString(R.string.stream_network_lost_reconnecting), noticeAfterLoss(game, overlay));
        } finally {
            cleanupRecovery(game);
        }
    }

    // Route problem checks delegate to nightjar_core::route::route_problem (whose unit tests cover
    // OS no-route errnos); verify JNI round-trip for reachable loopback and unresolvable host.
    @Test public void onlyAnImmediateNoRouteErrorWaitsForTheRoute() throws Exception {
        NightjarJni.ensureLibraryLoaded();
        assertNotNull(NightjarJni.routeProblem("invalid.invalid.", 47989));
        try (java.net.ServerSocket listener = new java.net.ServerSocket(0, 1, java.net.InetAddress.getLoopbackAddress())) {
            assertNull(NightjarJni.routeProblem("127.0.0.1", listener.getLocalPort()));
        }
    }

    @Test public void recoveryQueuesOneAttemptAndStoppingCancelsIt() {
        Game game = recoveryGame();
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
            assertNotNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertFalse(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
            ReflectionHelpers.callInstanceMethod(game.streamConnection, "stopConnection");
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(15));
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }

    @Test public void noRecoveryInBackgroundAfterQuitOrWhenDisabled() {
        Game game = recoveryGame();
        ReflectionHelpers.setField(game, "foreground", false);
        assertFalse(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", false);
        assertFalse(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
        ReflectionHelpers.setField(game, "connected", true);
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        assertFalse(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
    }

    @Test public void lifecycleCancellationRemovesRetryAndDeadlineCallbacks() {
        Game game = recoveryGame();
        AtomicInteger ran = new AtomicInteger();
        Runnable retry = ran::incrementAndGet;
        Handler handler = ReflectionHelpers.getField(game, "bitrateHandler");
        ReflectionHelpers.setField(game, "pendingRecovery", retry);
        handler.postDelayed(retry, 1000);
        ReflectionHelpers.callInstanceMethod(game.streamRecovery, "cancelAutomaticRecovery");
        org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(2));
        assertEquals(0, ran.get());
        assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
    }

    @Test public void cancellingWhileTeardownIsRunningCannotLaunchAnotherActivity() throws Exception {
        Game game = recoveryGame();
        java.util.concurrent.CountDownLatch stopEntered = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch stopReleased = new java.util.concurrent.CountDownLatch(1);
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null) {
            @Override public void stop() {
                stopEntered.countDown();
                try { stopReleased.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
            assertTrue(stopEntered.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
            // The delayed retry is now waiting for teardown on a worker thread.
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            ReflectionHelpers.callInstanceMethod(game.streamConnection, "stopConnection");
            stopReleased.countDown();
            // Any late posted continuation must observe the cancellation generation.
            settleHandoff(game);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            stopReleased.countDown();
            ReflectionHelpers.callInstanceMethod(game.streamRecovery, "cancelAutomaticRecovery");
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }

    @Test public void pauseResumeDuringFirstRecoveryTeardownShowsManualRecoveryInsteadOfStrandedSpinner() throws Exception {
        assertPauseDuringHandoffShowsManualRecovery(true);
    }

    @Test public void automaticDisplayDetachUsesResumeOnlyEvenWhenNetworkRecoveryIsDisabled() throws Exception {
        Game game = recoveryGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        ReflectionHelpers.setField(game, "recoveryAppId", 73);
        ReflectionHelpers.setField(game, "recoveryAppUuid", "captured-game");
        try {
            ReflectionHelpers.callInstanceMethod(game.videoOutput, "returnVideoToDevice");
            android.content.Intent retry = awaitStartedActivity(game);
            assertEquals(73, retry.getIntExtra(Game.EXTRA_APP_ID, 0));
            assertEquals("captured-game", retry.getStringExtra(Game.EXTRA_APP_UUID));
            assertFalse(retry.hasExtra("rubylight.outputDisplay"));
            Game replacement = Robolectric.buildActivity(Game.class).get();
            replacement.setIntent(retry);
            assertTrue(ReflectionHelpers.callInstanceMethod(replacement.streamRecovery, "isResumeOnlyStream"));
            assertFalse(ReflectionHelpers.callInstanceMethod(replacement.streamRecovery, "isRecoveryAttempt"));
        } finally {
            ReflectionHelpers.callInstanceMethod(game.streamRecovery, "cancelAutomaticRecovery");
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }

    @Test public void pauseResumeDuringOutputReconnectShowsManualRecoveryInsteadOfStrandedSpinner() throws Exception {
        assertPauseDuringHandoffShowsManualRecovery(false);
    }

    private void assertPauseDuringHandoffShowsManualRecovery(boolean automatic) throws Exception {
        Game game = recoveryGame();
        java.util.concurrent.CountDownLatch stopEntered = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch stopReleased = new java.util.concurrent.CountDownLatch(1);
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null) {
            @Override public void stop() {
                stopEntered.countDown();
                try { stopReleased.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                catch (InterruptedException interrupted) { Thread.currentThread().interrupt(); }
            }
        });
        try {
            if (automatic) assertTrue(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
            else ReflectionHelpers.callInstanceMethod(game.streamRecovery, "reconnectStream");
            assertTrue(stopEntered.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertFalse(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "isRecoveryAttempt"));
            assertEquals(Boolean.TRUE, ReflectionHelpers.getField(game, "recoveryHandoffInProgress"));
            game.onPause();
            assertEquals(Boolean.TRUE, ReflectionHelpers.getField(game, "recoveryCancelledOnPause"));
            game.onResume();
            assertNull(ReflectionHelpers.getField(game, "spinner"));
            android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull(dialog); assertTrue(dialog.isShowing());
            assertEquals(Boolean.FALSE, ReflectionHelpers.getField(game, "recoveryHandoffInProgress"));
            stopReleased.countDown();
            settleHandoff(game);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            stopReleased.countDown();
            ReflectionHelpers.callInstanceMethod(game.streamRecovery, "cancelAutomaticRecovery");
            android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            if (dialog != null) dialog.dismiss();
            com.limelight.utils.SpinnerDialog.closeDialogs(game);
        }
    }
}
