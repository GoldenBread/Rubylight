package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.os.SystemClock;

import com.limelight.GameRecoveryTestSupport.MuteConnection;
import com.limelight.GameRecoveryTestSupport.RecoveryBridge;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static com.limelight.GameRecoveryTestSupport.*;
import static org.junit.Assert.*;

/**
 * Tests Continue Here stream-move detection, move-watch polling, "Stream here too" joins,
 * who-plays prompts, and PIN-lock resume flows in {@link GameRecovery}.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameRecoveryMoveTest {
    /** "Stream here too" on "did not move", with the PC giving this answer when it is pressed. */
    private void streamTooHears(String answer, java.util.function.Consumer<Intent> expect) throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, PRESS_FAILED, answer));
        try {
            awaitReads(reads, 1);
            assertEquals(game.getString(R.string.continue_here_did_not_move), promptTitle(game));
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            expect.accept(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    /** Resume on "the game came back", with the PC giving this answer when it is pressed. */
    private void cameBackHears(String answer, java.util.function.Consumer<Intent> expect) throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, GAME_BACK, answer));
        try {
            awaitReads(reads, 1);
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            expect.accept(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aRecoveryThatWasExpectedButRefusedStillShowsTheMove() throws Exception {
        Game game = establishedGame();
        java.util.List<Boolean> asked = new java.util.ArrayList<>();
        ReflectionHelpers.setField(game, "conn", movedTo("TV", asked));
        long now = SystemClock.elapsedRealtime();
        setBudget(game, NightjarPolicyJni.recoveryMaxAttempts(), now);
        ReflectionHelpers.setField(game.streamRecovery, "connectedAt", now);
        try {
            game.connectionTerminated(-1);
            assertNull("the UI thread does not wait for the read", ReflectionHelpers.getField(game, "streamMenu"));
            awaitOnUi(() -> ReflectionHelpers.getField(game, "streamMenu") != null);
            assertEquals("told first that a recovery follows, then asked", java.util.List.of(true, false), asked);
            assertMovedPrompt(game, game.getString(R.string.continue_here_now_playing, "TV"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aRecoveryThatWasExpectedButRefusedWithoutAMoveShowsTheError() throws Exception {
        Game game = establishedGame();
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public boolean offersContinueHere() { return true; }
            @Override public String streamMovedTo(boolean recoveryFollows) { return null; }
            @Override public boolean hasEndedSession() { return false; }
        });
        long now = SystemClock.elapsedRealtime();
        setBudget(game, NightjarPolicyJni.recoveryMaxAttempts(), now);
        ReflectionHelpers.setField(game.streamRecovery, "connectedAt", now);
        try {
            game.connectionTerminated(-1);
            awaitOnUi(() -> ReflectionHelpers.getField(game, "streamMenu") != null);
            assertFalse(game.isFinishing());
            assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aMoveThatFailsTurnsThePromptIntoAnOfferToResume() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, STILL_MOVED, null, GAME_BACK));
        try {
            assertMovedPrompt(game, game.getString(R.string.continue_here_now_playing, "TV"));
            assertEquals("nothing is read on the UI thread, nor at once", 0, reads.get());
            awaitReads(reads, 3);
            assertEquals(game.getString(R.string.continue_here_came_back), promptTitle(game));
            int after = reads.get();
            passWatchInterval();
            assertEquals("nothing left to watch", after, reads.get());

            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            Intent started = org.robolectric.Shadows.shadowOf(game).getNextStartedActivity();
            assertEquals(Game.class.getName(), started.getComponent().getClassName());
            assertTrue("a game that ended meanwhile is not started again", started.getBooleanExtra("rubylight.outputResumeOnly", false));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aReadThatFailsOrStillSaysMovedChangesNothing() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, STILL_MOVED, null));
        try {
            awaitReads(reads, 3);
            assertMovedPrompt(game, game.getString(R.string.continue_here_now_playing, "TV"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void streamHereTooJoinsWhileTheGameStaysAnotherDevices() throws Exception {
        streamTooHears(PRESS_FAILED, GameRecoveryTestSupport::assertJoins);
        streamTooHears(TV_PLAYS, GameRecoveryTestSupport::assertJoins);
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void streamHereTooOnAPcThatDoesNotAnswerOrWhoseGameCameBackNeverJoins() throws Exception {
        // /resume joins whatever runs: no answer is no reason to.
        streamTooHears(null, GameRecoveryTestSupport::assertResumesOnly);
        streamTooHears(GAME_BACK, GameRecoveryTestSupport::assertResumesOnly);
        streamTooHears("{}", GameRecoveryTestSupport::assertResumesOnly);
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void resumingAGameThatTurnedOutAnotherDevicesWithNobodyToAskAboutJoins() throws Exception {
        cameBackHears(PRESS_FAILED, GameRecoveryTestSupport::assertJoins);
        cameBackHears(null, GameRecoveryTestSupport::assertResumesOnly);
        cameBackHears(GAME_BACK, GameRecoveryTestSupport::assertResumesOnly);
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void underThePinLockAJoinWaitsBehindThePinScreen() throws Exception {
        com.limelight.pin.PinGate.resetForTests();
        Application application = org.robolectric.RuntimeEnvironment.getApplication();
        com.limelight.pin.PinStore store = new com.limelight.pin.PinStore(application);
        store.setPin("2468".getBytes(java.nio.charset.StandardCharsets.US_ASCII));
        store.setMode(com.limelight.pin.PinStore.Mode.STREAMS);
        try {
            streamTooHears(PRESS_FAILED, started -> {
                assertEquals(com.limelight.pin.PinLockActivity.class.getName(), started.getComponent().getClassName());
                assertJoins(started.getParcelableExtra(com.limelight.pin.PinLockActivity.EXTRA_NEXT));
            });
        } finally {
            store.setMode(com.limelight.pin.PinStore.Mode.OFF);
            com.limelight.pin.PinGate.resetForTests(); com.limelight.pin.PinStore.clearForTests(application);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aRecoveryOfAJoinNeverJoinsAgain() throws Exception {
        Game game = recoveryGame();
        game.getIntent().putExtra(GameRecovery.EXTRA_JOIN_RUNNING, true);
        try {
            assertTrue(ReflectionHelpers.callInstanceMethod(game.streamRecovery, "tryAutomaticRecovery"));
            assertFalse(awaitRecoveryLaunch(game, 1).hasExtra(GameRecovery.EXTRA_JOIN_RUNNING));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void leavingWhileThePcSaysWhoPlaysLeavesNoSpinnerAndAsksOnTheWayBack() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, GAME_BACK));
        try {
            awaitReads(reads, 1);
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            game.onPause();
            awaitReads(reads, 2);
            assertNull("no spinner that nothing ends", ReflectionHelpers.getField(game, "spinner"));
            assertFalse(ReflectionHelpers.<Boolean>getField(game, "recoveryHandoffInProgress"));
            assertTrue(ReflectionHelpers.<Boolean>getField(game, "recoveryCancelledOnPause"));
            game.onResume();
            assertEquals(game.getString(R.string.stream_recovery_paused), promptMessage(game));
            // Its Reconnect stands in for the press it replaced, which could not start a closed game.
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            assertResumesOnly(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void leavingWhileTheStreamMovesBackLeavesNoSpinnerAndAsksOnTheWayBack() throws Exception {
        java.util.concurrent.CountDownLatch pressed = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch answer = new java.util.concurrent.CountDownLatch(1);
        com.limelight.nativeapi.NativeApiClient pc = new com.limelight.nativeapi.NativeApiClient((host, op, argument) -> {
            if (op.equals("session_continue_here")) {
                pressed.countDown();
                try { answer.await(HANG_GUARD_MS, java.util.concurrent.TimeUnit.MILLISECONDS); }
                catch (InterruptedException e) { Thread.currentThread().interrupt(); }
            }
            return "{\"ok\":" + STILL_MOVED + "}";
        }, new com.limelight.nativeapi.HostEndpoint("pc", "192.0.2.1", 47989, 47984, "pem", "device", new byte[0], new byte[0]));
        Game game = movedAwayFrom(pc);
        try {
            promptButton(game, android.content.DialogInterface.BUTTON_NEGATIVE).performClick();
            awaitOnUi(() -> pressed.getCount() == 0);
            game.onPause();
            answer.countDown();
            for (int i = 0; i < 40; i++) {
                Thread.sleep(5);
                org.robolectric.shadows.ShadowLooper.idleMainLooper();
            }
            assertNull("no spinner that nothing ends", ReflectionHelpers.getField(game, "spinner"));
            assertFalse(ReflectionHelpers.<Boolean>getField(game, "recoveryHandoffInProgress"));
            assertTrue(ReflectionHelpers.<Boolean>getField(game, "recoveryCancelledOnPause"));
            game.onResume();
            assertEquals(game.getString(R.string.stream_recovery_paused), promptMessage(game));
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            assertResumesOnly(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            answer.countDown();
            cleanupRecovery(game);
        }
    }

    @Test
    public void aStartAsksToJoinOnlyOnce() {
        Game game = recoveryGame();
        game.getIntent().putExtra(GameRecovery.EXTRA_JOIN_RUNNING, true);
        assertTrue(game.streamRecovery.isJoinRunningStream());
        assertFalse("a re-created screen checks what it finds", game.streamRecovery.isJoinRunningStream());
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aStreamStartedFromATileAsksByTheGameItStreamed() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, GAME_BACK, TV_PLAYS));
        // The Resume tile's own identity; the game it streamed is 73.
        ReflectionHelpers.setField(game, "app", new com.limelight.nvstream.http.NvApp("Resume", 4242, false));
        try {
            awaitReads(reads, 1);
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            String asked = game.getString(R.string.continue_here_playing_on, "Game", "TV");
            awaitOnUi(() -> asked.equals(promptMessage(game)) || game.isFinishing());
            assertFalse("it asks instead of starting", game.isFinishing());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void takingTheStreamBackNeverStartsAGameThatClosedMeanwhile() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, STILL_MOVED));
        try {
            promptButton(game, android.content.DialogInterface.BUTTON_NEGATIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            assertResumesOnly(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void takingTheStreamBackWithNoGameToNameSaysSoAndLeaves() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, STILL_MOVED));
        ReflectionHelpers.setField(game, "app", null);
        try {
            promptButton(game, android.content.DialogInterface.BUTTON_NEGATIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
            assertEquals(game.getString(R.string.stream_recovery_session_changed),
                    org.robolectric.shadows.ShadowToast.getTextOfLatestToast());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void resumingAsksWhoPlaysWhenTheOtherDeviceArrivedAfterAll() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, GAME_BACK, TV_PLAYS));
        try {
            awaitReads(reads, 1);
            assertEquals(game.getString(R.string.continue_here_came_back), promptTitle(game));
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            String asked = game.getString(R.string.continue_here_playing_on, "Game", "TV");
            awaitOnUi(() -> asked.equals(promptMessage(game)));
            assertFalse("nothing started without an answer", game.isFinishing());
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());

            promptButton(game, android.content.DialogInterface.BUTTON_NEUTRAL).performClick();
            awaitOnUi(() -> game.isFinishing());
            assertJoins(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void cancellingWhoPlaysLeavesWithoutAStart() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, GAME_BACK, TV_PLAYS));
        try {
            awaitReads(reads, 1);
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            String asked = game.getString(R.string.continue_here_playing_on, "Game", "TV");
            awaitOnUi(() -> asked.equals(promptMessage(game)));
            promptButton(game, android.content.DialogInterface.BUTTON_NEGATIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(game.isFinishing());
            assertNull(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aMoveThatFailsOnADeviceThatOnlyStreamedBesideOffersThatAgain() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, "{\"press_failed\":true}"));
        try {
            awaitReads(reads, 1);
            assertEquals(game.getString(R.string.continue_here_did_not_move), promptTitle(game));
            android.widget.Button too = promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE);
            assertEquals(game.getString(R.string.continue_here_stream_too), too.getText().toString());
            int after = reads.get();
            passWatchInterval();
            assertEquals("nothing left to watch", after, reads.get());
            too.performClick();
            awaitOnUi(() -> game.isFinishing());
            assertJoins(org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void okOnTheOfferJustLeaves() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, GAME_BACK));
        try {
            awaitReads(reads, 1);
            assertEquals(game.getString(R.string.continue_here_came_back), promptTitle(game));
            promptButton(game, android.content.DialogInterface.BUTTON_NEGATIVE).performClick();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(game.isFinishing());
            assertNull("nothing reconnects", org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void underThePinLockTheOfferResumesBehindThePinScreen() throws Exception {
        com.limelight.pin.PinGate.resetForTests();
        Application application = org.robolectric.RuntimeEnvironment.getApplication();
        com.limelight.pin.PinStore store = new com.limelight.pin.PinStore(application);
        store.setPin("2468".getBytes(java.nio.charset.StandardCharsets.US_ASCII));
        store.setMode(com.limelight.pin.PinStore.Mode.STREAMS);
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, GAME_BACK));
        try {
            assertNull("the moved prompt itself has only OK",
                    promptButton(game, android.content.DialogInterface.BUTTON_NEGATIVE).getVisibility() == android.view.View.VISIBLE ? "shown" : null);
            awaitReads(reads, 1);
            assertEquals(game.getString(R.string.continue_here_came_back), promptTitle(game));
            promptButton(game, android.content.DialogInterface.BUTTON_POSITIVE).performClick();
            awaitOnUi(() -> game.isFinishing());
            Intent started = org.robolectric.Shadows.shadowOf(game).getNextStartedActivity();
            assertEquals(com.limelight.pin.PinLockActivity.class.getName(), started.getComponent().getClassName());
            Intent next = started.getParcelableExtra(com.limelight.pin.PinLockActivity.EXTRA_NEXT);
            assertEquals("the PIN screen starts the stream", Game.class.getName(), next.getComponent().getClassName());
            assertTrue(next.getBooleanExtra("rubylight.outputResumeOnly", false));
        } finally {
            cleanupRecovery(game);
            store.setMode(com.limelight.pin.PinStore.Mode.OFF);
            com.limelight.pin.PinGate.resetForTests(); com.limelight.pin.PinStore.clearForTests(application);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aNoteThatRanOutWithTheGameElsewhereOffersNothingAndEndsTheWatch() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, "{\"owner\":\"other_device\"}"));
        try {
            awaitReads(reads, 1);
            assertMovedPrompt(game, game.getString(R.string.continue_here_now_playing, "TV"));
            passWatchInterval();
            assertEquals(1, reads.get());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aClosedPromptEndsTheWatch() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, STILL_MOVED));
        try {
            awaitReads(reads, 1);
            int before = reads.get();
            android.app.Dialog prompt = ReflectionHelpers.getField(game, "streamMenu");
            prompt.dismiss();
            for (int i = 0; i < 3; i++) passWatchInterval();
            Thread.sleep(50);
            assertEquals(before, reads.get());
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aPcThatNeverAnswersIsNotAskedForever() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, (String) null));
        try {
            awaitReads(reads, GameConnection.MOVE_WATCH_READS);
            passWatchInterval();
            Thread.sleep(50);
            assertEquals(GameConnection.MOVE_WATCH_READS, reads.get());
            assertMovedPrompt(game, game.getString(R.string.continue_here_now_playing, "TV"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aLostConnectionThatRecoversDoesNotWaitForTheMoveRead() {
        for (int error : new int[] {-1, MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC}) {
            Game game = establishedGame();
            java.util.List<Boolean> asked = new java.util.ArrayList<>();
            ReflectionHelpers.setField(game, "conn", movedTo("TV", asked));
            try {
                game.connectionTerminated(error);
                assertEquals("told that a recovery follows, so it does not read", java.util.List.of(true), asked);
                assertNotNull(ReflectionHelpers.getField(game.streamRecovery, "pendingRecoveryIntent"));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aMediaTimeoutWithoutRecoveryShowsTheMove() {
        for (int error : new int[] {-100, -101, -102, -1}) {
            Game game = establishedGame();
            PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
            prefs.autoReconnect = false;
            java.util.List<Boolean> asked = new java.util.ArrayList<>();
            ReflectionHelpers.setField(game, "conn", movedTo("TV", asked));
            try {
                game.connectionTerminated(error);
                assertEquals(java.util.List.of(false), asked);
                assertMovedPrompt(game, game.getString(R.string.continue_here_now_playing, "TV"));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aGracefulEndOfAStreamKeptForResumingShowsTheMove() {
        Game game = establishedGame();
        ReflectionHelpers.setField(game, "conn", movedTo("TV", new java.util.ArrayList<>()));
        try {
            game.onPause();
            game.onStop();
            assertNotNull(ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            game.connectionTerminated(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION);
            assertNull("it never resumes", ReflectionHelpers.getField(game, "stoppedStreamResumeIntent"));
            assertMovedPrompt(game, game.getString(R.string.continue_here_now_playing, "TV"));
        } finally {
            cleanupRecovery(game);
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void aResumeThatFindsTheMoveShowsThePromptAndNotAStartupError() {
        for (String device : new String[] {"TV", ""}) {
            Game game = establishedGame();
            try {
                game.streamMoved(device);
                game.stageFailed("Game", 0, 0);
                assertMovedPrompt(game, device.isEmpty()
                        ? game.getString(R.string.continue_here_now_playing_elsewhere)
                        : game.getString(R.string.continue_here_now_playing, device));
            } finally {
                cleanupRecovery(game);
            }
        }
    }

    @Test
    @Config(shadows = RecoveryBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
    public void dismissingOrStoppingMovedPromptDestroysMoveWatchHandleImmediately() throws Exception {
        java.util.concurrent.atomic.AtomicInteger reads = new java.util.concurrent.atomic.AtomicInteger();
        Game game = movedAwayFrom(pcAnswering(reads, STILL_MOVED));
        try {
            assertNotEquals("move watch handle created", 0L, game.streamConnection.moveWatchHandle);
            assertNotNull("move watch poll scheduled", game.streamConnection.moveWatchTask);
            // Dismissing the prompt (e.g., tapping OK) before the 3s timer fires must free the handle at once.
            android.app.Dialog prompt = ReflectionHelpers.getField(game, "streamMenu");
            assertNotNull(prompt);
            prompt.dismiss();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals("handle destroyed on dismiss", 0L, game.streamConnection.moveWatchHandle);
            assertNull("delayed poll cancelled on dismiss", game.streamConnection.moveWatchTask);
            passWatchInterval();
            assertEquals("no background read after dismiss", 0, reads.get());
        } finally {
            cleanupRecovery(game);
        }

        // Also verify stopConnection() destroys the handle directly even if the prompt did not dismiss first.
        Game destroyed = movedAwayFrom(pcAnswering(reads, STILL_MOVED));
        try {
            assertNotEquals(0L, destroyed.streamConnection.moveWatchHandle);
            destroyed.streamConnection.stopConnection();
            assertEquals(0L, destroyed.streamConnection.moveWatchHandle);
            assertNull(destroyed.streamConnection.moveWatchTask);
            passWatchInterval();
            assertEquals(0, reads.get());
        } finally {
            cleanupRecovery(destroyed);
        }
    }
}
