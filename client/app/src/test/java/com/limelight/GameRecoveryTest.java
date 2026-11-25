package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.os.SystemClock;

import com.limelight.GameRecoveryTestSupport.MuteConnection;
import com.limelight.GameRecoveryTestSupport.RecoveryBridge;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.nvstream.jni.MoonBridge;
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
 * Core tests for {@link GameRecovery}: recovery text/bitrate policy, microphone mute preservation
 * across reconnects, startup/disconnect/quit classification, and media failure provenance.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameRecoveryTest {
    /** A recovery that gives up says why its last attempt failed, then that it stopped (desktop's recovery_gave_up). */
    @Test public void aRecoveryThatGivesUpSaysWhyTheLastAttemptFailed() {
        String stopped = RuntimeEnvironment.getApplication().getString(R.string.stream_recovery_stopped);
        assertEquals("Starting the stream failed.\n\n" + stopped, GameRecovery.gaveUpText("Starting the stream failed.", stopped));
        assertEquals(stopped, GameRecovery.gaveUpText(null, stopped));
        assertEquals(stopped, GameRecovery.gaveUpText("", stopped));
        // A resume from the background that finds no attempts left names the failure too, else the unreachable PC.
        String unreachable = RuntimeEnvironment.getApplication().getString(R.string.stream_recovery_unreachable);
        assertEquals("The connection was lost.", GameRecovery.gaveUpCause("The connection was lost.", unreachable));
        assertEquals(unreachable, GameRecovery.gaveUpCause(null, unreachable));
        assertEquals(unreachable, GameRecovery.gaveUpCause("", unreachable));
        assertFalse(RuntimeEnvironment.getApplication().getString(R.string.stream_recovery_attempt, 2, 5).contains("Back"));
    }

    /** A bitrate the player set starts every reconnect; an automatic cut is where an automatic one starts again. */
    @Test public void aReconnectKeepsTheBitrateTheStreamSettledOn() {
        Game game = Robolectric.buildActivity(Game.class, new Intent()).get();
        game.currentBitrate = 12000;
        game.adaptiveBitrate = new com.limelight.nvstream.AdaptiveBitrateController(30000, 12000, 60, 0, 1920, 1080, null);
        Intent automatic = new Intent();
        game.streamRecovery.putSessionBitrate(automatic, true);
        assertEquals(12000, automatic.getIntExtra(GameRecovery.EXTRA_RECOVERY_AUTO_BITRATE, 0));
        assertFalse(automatic.hasExtra(GameRecovery.EXTRA_RECOVERY_MANUAL_BITRATE));
        // A Reconnect the player pressed starts from the saved bitrate again.
        Intent manualPress = new Intent(automatic);
        game.streamRecovery.putSessionBitrate(manualPress, false);
        assertFalse(manualPress.hasExtra(GameRecovery.EXTRA_RECOVERY_AUTO_BITRATE));
        // The automatic start is taken once, and only lowers the encoder's rate.
        Game replacement = Robolectric.buildActivity(Game.class, automatic).get();
        assertEquals(12000, replacement.streamRecovery.takeAutomaticBitrateStart());
        assertEquals(0, replacement.streamRecovery.takeAutomaticBitrateStart());
        assertEquals(12000, GameRecovery.automaticBitrateStart(30000, 12000));
        assertEquals(20000, GameRecovery.automaticBitrateStart(20000, 30000));
        assertEquals(30000, GameRecovery.automaticBitrateStart(30000, 0));

        // A bitrate the player set wins over the cut and starts the new stream with automatic bitrate off.
        game.sessionManualBitrate = 8000;
        Intent manual = new Intent();
        game.streamRecovery.putSessionBitrate(manual, true);
        assertEquals(8000, manual.getIntExtra(GameRecovery.EXTRA_RECOVERY_MANUAL_BITRATE, 0));
        assertFalse(manual.hasExtra(GameRecovery.EXTRA_RECOVERY_AUTO_BITRATE));
        Game restored = Robolectric.buildActivity(Game.class, manual).get();
        // Not read from the preferences: that loads the native library, which a unit test has not.
        restored.prefConfig = new PreferenceConfiguration();
        restored.prefConfig.automaticBitrate = true;
        restored.streamRecovery.restoreSessionBitrate();
        assertEquals(8000, restored.prefConfig.bitrate);
        assertEquals(8000, restored.sessionManualBitrate);
        assertFalse(restored.prefConfig.automaticBitrate);
    }

    @Test public void mergedManifestAllowsReturningToAStoppedStream() throws Exception {
        Game game = Robolectric.buildActivity(Game.class).get();
        ActivityInfo info = game.getPackageManager().getActivityInfo(new ComponentName(game, Game.class), 0);
        assertEquals(0, info.flags & ActivityInfo.FLAG_NO_HISTORY);
    }

    @Test public void settingsAfterAShortcutsFailedStartHaveTheLibraryUnderThem() {
        Application app = org.robolectric.RuntimeEnvironment.getApplication();
        Intent start = new Intent(app, Game.class).putExtra(Game.EXTRA_PC_UUID, "pc-1").putExtra(Game.EXTRA_PC_NAME, "Desk");
        Intent[] alone = GameRecovery.settingsAfterFailedStart(app, start, true);
        assertEquals("a shortcut's stream had nothing under it: the library goes under Settings", 2, alone.length);
        assertEquals(AppView.class.getName(), alone[0].getComponent().getClassName());
        assertEquals("pc-1", alone[0].getStringExtra(AppView.UUID_EXTRA));
        assertEquals("Desk", alone[0].getStringExtra(AppView.NAME_EXTRA));
        assertEquals(com.limelight.preferences.StreamSettings.class.getName(), alone[1].getComponent().getClassName());
        Intent[] fromLibrary = GameRecovery.settingsAfterFailedStart(app, start, false);
        assertEquals("started from the library, which is still under it", 1, fromLibrary.length);
        assertEquals(com.limelight.preferences.StreamSettings.class.getName(), fromLibrary[0].getComponent().getClassName());
    }

    @Test public void automaticRecoveryRestoresLocalAndControllerHostMuteBeforeCapture() throws Exception {
        for (boolean controllerMuted : new boolean[] {false, true}) {
            Game game = recoveryGame();
            MuteConnection connection = new MuteConnection();
            ReflectionHelpers.setField(game, "conn", connection);
            if (controllerMuted) connection.controllerMicrophoneState(new Object(), false, false, true);
            else connection.setMicrophoneMuted(true);
            try {
                assertTrue(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
                Intent retry = awaitRecoveryLaunch(game, 1);
                assertRestoredMute(retry, true);
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test public void lockResumeKeepsHostMuteAfterTheStoppedConnectionClearsControllerState() throws Exception {
        Game game = establishedGame();
        MuteConnection connection = new MuteConnection();
        connection.controllerMicrophoneState(new Object(), false, false, true);
        ReflectionHelpers.setField(game, "conn", connection);
        try {
            game.onPause();
            game.onStop();
            Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
            join(stopping);
            assertFalse(connection.isMicrophoneMutedForReconnect());
            game.onResume();
            assertRestoredMute(awaitRecoveryLaunch(game, 1), true);
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test public void manualAndOutputReconnectPreserveTheCurrentMuteWithoutPersistingAnOldMute() {
        for (boolean outputSwitch : new boolean[] {false, true}) {
            Game game = recoveryGame();
            MuteConnection connection = new MuteConnection();
            ReflectionHelpers.setField(game, "conn", connection);
            ReflectionHelpers.setField(game, "outputSwitchPending", outputSwitch);
            connection.setMicrophoneMuted(true);
            assertRestoredMute(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent"), true);
            connection.setMicrophoneMuted(false);
            assertRestoredMute(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent"), false);
        }
        // A separate fresh launch still uses the unmuted default; the choice belongs to this session.
        assertRestoredMute(new Intent(), false);
    }

    @Test public void manualReconnectKeepsTheMuteSnapshotWhenTheStoppedConnectionIsGone() throws Exception {
        Game game = recoveryGame();
        MuteConnection connection = new MuteConnection();
        connection.controllerMicrophoneState(new Object(), false, false, true);
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.callInstanceMethod(game.streamConnection, "stopConnection");
        Thread stopping = ReflectionHelpers.getField(game, "connectionStopThread");
        join(stopping);
        assertFalse(connection.isMicrophoneMutedForReconnect());
        ReflectionHelpers.setField(game, "conn", null);
        assertRestoredMute(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent"), true);
    }

    @Test
    @Config(shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void finalControllerTeardownMuteReachesAlreadyPreparedRecoveryAndReconnectIntents() throws Exception {
        for (boolean automatic : new boolean[] {false, true}) {
            Game game = recoveryGame();
            MuteConnection connection = new MuteConnection();
            ReflectionHelpers.setField(game, "conn", connection);
            PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
            // This fixture has no Shield service; do not post a successful binding with a null binder.
            org.robolectric.Shadows.shadowOf(game.getApplication()).declareComponentUnbindable(
                    new ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
            ControllerHandler handler = new ControllerHandler(game, connection, null, config) {
                @Override public synchronized void stop() {
                    // A USB worker can publish its last host mute while being synchronously joined.
                    Object owner = new Object();
                    connection.controllerMicrophoneState(owner, false, false, true);
                    connection.releaseControllerMicrophone(owner);
                    super.stop();
                }
            };
            ReflectionHelpers.setField(game, "controllerHandler", handler);
            try {
                assertFalse(connection.isMicrophoneMutedForReconnect());
                if (automatic) assertTrue(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
                else ReflectionHelpers.callInstanceMethod(game.streamRecovery, "reconnectStream");
                assertRestoredMute(awaitRecoveryLaunch(game, automatic ? 1 : 0), true);
            } finally {
                cleanupRecovery(game);
                handler.destroy();
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void nativeStartupTimeoutQueuesNextRecoveryWithoutAConnectivityProbe() {
        Game game = recoveryGame();
        long started = SystemClock.elapsedRealtime();
        game.setIntent(new Intent().putExtra("RecoveryAttempts", 1).putExtra("RecoveryStarted", started));
        setBudget(game, 1, started);
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        RecoveryBridge.connectivityTests = 0;
        try {
            game.stageFailed("Control stream establishment", MoonBridge.ML_PORT_FLAG_UDP_47999, 110);
            assertNotNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertEquals(2, game.recoveryAttempts);
            assertEquals(0, RecoveryBridge.connectivityTests);
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
    }

    // The review's regression: video showed, the network dropped, and the first reconnect had no frame
    // yet when it dropped too. The chain carries its video, so the next attempt follows; a stream whose
    // chain never showed video shows its error at once, and a network return does not reconnect it.
    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aReconnectWithoutAFrameYetKeepsTheChainGoingAfterARealDrop() throws Exception {
        Game shown = establishedGame();
        ReflectionHelpers.setField(shown, "conn", new MuteConnection());
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(shown.streamRecovery, "tryAutomaticRecovery"));
            assertTrue("the drop came after video", awaitRecoveryLaunch(shown, 1).getBooleanExtra("RecoveryVideoShown", false));
        } finally {
            cleanupRecovery(shown);
        }
        for (boolean chainHadVideo : new boolean[] {true, false}) {
            Game attempt = establishedGame();
            ReflectionHelpers.setField(ReflectionHelpers.getField(attempt, "decoderRenderer"), "lastVideoFrameTimeMs", 0L);
            attempt.setIntent(new Intent().putExtra("RecoveryVideoShown", chainHadVideo));
            ReflectionHelpers.setField(attempt, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { return false; }
            });
            try {
                attempt.connectionTerminated(MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC);
                assertEquals(chainHadVideo, ReflectionHelpers.getField(attempt, "pendingRecovery") != null);
            } finally {
                cleanupRecovery(attempt);
            }
        }
        for (boolean video : new boolean[] {true, false}) {
            Game returned = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(returned, "prefConfig");
            prefs.networkGraceSeconds = 0;
            if (!video) {
                ReflectionHelpers.setField(ReflectionHelpers.getField(returned, "decoderRenderer"), "lastVideoFrameTimeMs", 0L);
            }
            ReflectionHelpers.setField(returned, "conn", new MuteConnection());
            try {
                returned.streamNetworkReturned(MoonBridge.NETWORK_CANNOT_CONTINUE, false);
                org.robolectric.shadows.ShadowLooper.idleMainLooper();
                assertEquals("a network return reconnects only a stream that had video",
                        video, ReflectionHelpers.getField(returned.streamRecovery, "pendingRecoveryIntent") != null);
            } finally {
                cleanupRecovery(returned);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void confirmedHostExitFinishesWithoutAnErrorOrRetryWithRecoveryOnOrOff() {
        for (boolean recoveryEnabled : new boolean[] {false, true}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = recoveryEnabled;
            AtomicInteger probes = new AtomicInteger();
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { probes.incrementAndGet(); return true; }
            });
            RecoveryBridge.connectivityTests = 0;
            try {
                game.connectionTerminated(-1);
                assertEquals(1, probes.get());
                assertTrue(game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertEquals(0, RecoveryBridge.connectivityTests);
                assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void unconfirmedDisconnectStillRetriesOrShowsTheOriginalErrorAndKeepsItsReason() {
        for (boolean recoveryEnabled : new boolean[] {false, true}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = recoveryEnabled;
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { return false; }
            });
            try {
                game.connectionTerminated(-1);
                // With recovery off the stream closes and says why in a message that outlives it.
                assertEquals(!recoveryEnabled, game.isFinishing());
                assertEquals(recoveryEnabled, ReflectionHelpers.getField(game, "pendingRecovery") != null);
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                if (!recoveryEnabled) {
                    String toast = org.robolectric.shadows.ShadowToast.getTextOfLatestToast();
                    assertEquals("Connection Terminated\nError code: -1", toast);
                }
                assertTrue(LimeLog.getRedactedLog().contains("Connection terminated: -1"));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void quittingFromTheStreamMenuEndsQuietlyWhenTheHostDropsTheStream() throws Exception {
        for (boolean recoveryEnabled : new boolean[] {false, true}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = recoveryEnabled;
            java.util.concurrent.CountDownLatch quitting = new java.util.concurrent.CountDownLatch(1);
            java.util.concurrent.CountDownLatch answered = new java.util.concurrent.CountDownLatch(1);
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public void quitApp() {
                    quitting.countDown();
                    // The host tears the stream down before /cancel answers.
                    try { answered.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                    catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                }
                // The game is still exiting, so the host does not look idle yet.
                @Override public boolean hasEndedSession() { return false; }
            });
            RecoveryBridge.connectivityTests = 0;
            try {
                ReflectionHelpers.callInstanceMethod(game.menuActions, "quitStreamApp");
                assertTrue(quitting.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
                game.connectionTerminated(-1);
                assertTrue(game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertEquals(0, RecoveryBridge.connectivityTests);
                assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            } finally {
                answered.countDown();
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aFailedQuitRequestLeavesLaterFailuresVisible() throws Exception {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        java.util.concurrent.CountDownLatch refused = new java.util.concurrent.CountDownLatch(1);
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public void quitApp() throws java.io.IOException {
                refused.countDown();
                throw new java.io.IOException("The host did not quit the app.");
            }
            @Override public boolean hasEndedSession() { return false; }
        });
        try {
            ReflectionHelpers.callInstanceMethod(game.menuActions, "quitStreamApp");
            assertTrue(refused.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
            awaitHostAction(game);
            // The stream outlived the time a dropped stream takes to report.
            org.robolectric.shadows.ShadowLooper.idleMainLooper(16, java.util.concurrent.TimeUnit.SECONDS);
            game.connectionTerminated(-1);
            // Not a graceful exit: the reason shows in a message that outlives the stream.
            assertTrue(game.isFinishing());
            String toast = org.robolectric.shadows.ShadowToast.getTextOfLatestToast();
            assertTrue(toast, toast.equals("Connection Terminated\nError code: -1"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aRequestThatFailsAfterTheHostDroppedTheStreamStillEndsQuietly() throws Exception {
        for (String request : new String[] {"quitStreamApp", "endRemoteSession"}) {
            for (boolean recoveryEnabled : new boolean[] {false, true}) {
                Game game = establishedGame();
                PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
                prefs.autoReconnect = recoveryEnabled;
                java.util.concurrent.CountDownLatch failed = new java.util.concurrent.CountDownLatch(1);
                ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                    // The host ended the session and dropped the stream, then the reply was lost.
                    @Override public void quitApp() throws java.io.IOException {
                        failed.countDown();
                        throw new java.net.SocketException("Connection reset");
                    }
                    @Override public void disconnectRemoteSession() throws java.io.IOException {
                        failed.countDown();
                        throw new java.net.SocketException("Connection reset");
                    }
                    // The game is still exiting, so the host does not look idle yet.
                    @Override public boolean hasEndedSession() { return false; }
                });
                RecoveryBridge.connectivityTests = 0;
                try {
                    ReflectionHelpers.callInstanceMethod(game.menuActions, request);
                    assertTrue(failed.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS));
                    awaitHostAction(game);
                    // The lost transport reports only after its timeout, well after the failure.
                    org.robolectric.shadows.ShadowLooper.idleMainLooper(9, java.util.concurrent.TimeUnit.SECONDS);
                    game.connectionTerminated(-1);
                    assertTrue(request + " ended in an error or a reconnect instead of closing", game.isFinishing());
                    assertNull(request, ReflectionHelpers.getField(game, "pendingRecovery"));
                    assertNull(request, ReflectionHelpers.getField(game, "streamMenu"));
                    assertEquals(0, RecoveryBridge.connectivityTests);
                    assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
                } finally {
                    cleanupRecovery(game);
                }
            }
        }
    }

    @Test public void aReasonlessDisconnectIsNotAttributedToThePc() {
        assertFalse(Game.terminationDescription(-1).contains("reported by the PC"));
        assertTrue(Game.terminationDescription(-1).startsWith("-1 ("));
        assertTrue(Game.terminationDescription(0x80030042).contains("reported by the PC"));
        assertTrue(Game.terminationDescription(MoonBridge.CLIENT_ERROR_VULKAN_RENDERER).contains("Vulkan"));
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aSessionThePcEndsAfterLeavingIsNeverResumed() throws Exception {
        Game game = establishedGame();
        try {
            game.onPause();
            game.onStop();
            assertNotNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            // The termination callback was already on its way when the stream stopped.
            game.connectionTerminated(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION);
            assertTrue(game.isFinishing());
            game.onResume();
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            org.robolectric.shadows.ShadowLooper.idleMainLooper(15, java.util.concurrent.TimeUnit.SECONDS);
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void explicitHostFailuresNeverBecomeGracefulExitsEvenWhenTheHostIsIdle() {
        for (int error : new int[] {MoonBridge.ML_ERROR_PROTECTED_CONTENT, MoonBridge.ML_ERROR_FRAME_CONVERSION}) {
            Game game = establishedGame();
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { throw new AssertionError("Explicit failure probed the host"); }
            });
            try {
                game.connectionTerminated(error);
                assertFalse(game.isFinishing());
                assertNotNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aReasonOnlyTheHostKnowsIsNotProbedAndStillReturnsToTheList() {
        Game game = establishedGame();
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { throw new AssertionError("Explicit failure probed the host"); }
        });
        try {
            game.connectionTerminated(503);
            assertTrue(game.isFinishing());
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertEquals("Connection Terminated\nError code: 503",
                    org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aHostShutdownOrLostConnectionReturnsToTheListWithAMessageInsteadOfADialog() {
        // -1 is an ENet disconnect, 110 a connection timeout, 503 and 0x80030020 reasons from the PC.
        for (int error : new int[] {-1, 110, 10054, 503, 0x80030020}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = false;
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { return false; }
            });
            try {
                game.connectionTerminated(error);
                assertTrue("code " + error, game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                // Android 12+ shows two lines of a text toast: the title and the code.
                String code = Math.abs(error) > 1000 ? Integer.toHexString(error) : Integer.toString(error);
                assertEquals("code " + error, "Connection Terminated\nError code: " + code,
                        org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void usedUpAutomaticReconnectingKeepsTheDialogWithItsReconnectButton() {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = true;
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { return false; }
        });
        long now = SystemClock.elapsedRealtime();
        setBudget(game, NightjarPolicyJni.recoveryMaxAttempts(), now);
        ReflectionHelpers.setField(game.streamRecovery, "connectedAt", now);
        try {
            game.connectionTerminated(-1);
            assertFalse(game.isFinishing());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull(dialog);
            assertTrue(dialog.isShowing());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aGraceTimeThatMayHaveRunOutKeepsTheDialogForItsAdvice() {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        prefs.networkGraceSeconds = 30;
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean hasEndedSession() { return false; }
        });
        try {
            game.connectionTerminated(-1);
            assertFalse(game.isFinishing());
            assertNotNull(ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void mediaFailuresKeepTheirRecoveryBehaviorWithoutBeingReclassifiedAsHostExits() throws Exception {
        for (int error : new int[] {MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC, MoonBridge.ML_ERROR_NO_VIDEO_FRAME,
                MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION}) {
            Game game = establishedGame();
            ReflectionHelpers.setField(game, "conn", new MuteConnection() {
                @Override public boolean hasEndedSession() { throw new AssertionError("Media failure probed the host"); }
            });
            try {
                game.connectionTerminated(error);
                assertFalse(game.isFinishing());
                assertNull(ReflectionHelpers.getField(game, "streamMenu"));
                assertNotNull(ReflectionHelpers.getField(game, "pendingRecovery"));
                assertTrue(awaitRecoveryLaunch(game, 1).getBooleanExtra("RecoveryMediaFailure", false));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void mediaFailureProvenanceSurvivesStartupRetriesUntilAManualReconnect() throws Exception {
        Game game = recoveryGame();
        game.setIntent(new Intent().putExtra("RecoveryAttempts", 1).putExtra("RecoveryMediaFailure", true));
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        try {
            game.stageFailed("Game", MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_TCP_47989, 0);
            assertTrue(awaitRecoveryLaunch(game, 1).getBooleanExtra("RecoveryMediaFailure", false));
            Intent manual = ReflectionHelpers.callInstanceMethod(game.streamRecovery, "createReconnectIntent");
            assertFalse(manual.hasExtra("RecoveryMediaFailure"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void anEndedSessionDuringRecoveryStartupFinishesWithoutAnotherAttempt() {
        Game game = recoveryGame();
        game.setIntent(new Intent().putExtra("RecoveryAttempts", 1));
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        try {
            game.connectionTerminated(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION);
            assertTrue(game.isFinishing());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
            assertNull(ReflectionHelpers.getField(game, "streamMenu"));
        } finally {
            cleanupRecovery(game);
        }
    }
}
