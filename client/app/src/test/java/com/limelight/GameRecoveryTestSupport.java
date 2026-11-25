package com.limelight;

import android.content.Intent;
import android.os.SystemClock;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.robolectric.Robolectric;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/**
 * Shared Robolectric shadows, fake connections, and lifecycle/handoff helpers for the
 * {@link GameRecovery} test suite.
 */
final class GameRecoveryTestSupport {
    static final String MICROPHONE_MUTED = "rubylight.microphoneMuted";
    /** How long a background step may take before the test calls it hung; a loaded machine stays well inside it. */
    static final long HANG_GUARD_MS = 10_000;
    /** Game's workers that wait for the stream's stop, then post the hand-off to the UI thread. */
    static final java.util.Set<String> HANDOFF_WORKERS =
            java.util.Set.of("Stream reconnect", "Automatic stream recovery");
    static final String STILL_MOVED = "{\"moved_to\":{\"device\":\"TV\",\"since_unix_ms\":5}}";
    static final String GAME_BACK = "{\"owns_game\":true}";
    static final String PRESS_FAILED = "{\"press_failed\":true}";
    static final String TV_PLAYS = "{\"owns_game\":false,\"moved_to\":{\"device\":\"TV\",\"since_unix_ms\":5},\"playing_on\":{\"device\":\"TV\","
            + "\"app\":{\"install_id\":\"\",\"app_id\":73,\"title\":\"Game\"}}}";

    private GameRecoveryTestSupport() {}

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class RecoveryBridge {
        static int connectivityTests;
        @Implementation protected static void __staticInitializer__() {}
        @Implementation public static int testClientConnectivity(String server, int port, int portFlags) {
            connectivityTests++;
            return 0;
        }
        @Implementation public static String stringifyPortFlags(int portFlags, String separator) {
            return Integer.toString(portFlags);
        }
        @Implementation public static int getPortFlagsFromTerminationErrorCode(int errorCode) { return 0; }
    }

    static class MuteConnection extends NvConnection {
        MuteConnection() {
            super(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().setApp(new NvApp("Game", 73, false)).build(), null, null);
        }

        @Override public void stop() {
            // Match native teardown's state removal without opening a socket, device, or microphone.
            java.util.Map<?, ?> microphones = ReflectionHelpers.getField(this, "controllerMicrophones");
            microphones.clear();
        }

        @Override public void sendControllerInput(short number, short mask, int buttons, byte leftTrigger,
                                                   byte rightTrigger, short leftX, short leftY, short rightX, short rightY) { }
    }

    /** Whether the stream screen was finishing when each activity it started was started. */
    static final class StartOrder {
        final java.util.List<Boolean> finishingAtStart = new java.util.ArrayList<>();
        private final android.app.Instrumentation instrumentation;
        private final android.app.Instrumentation.ActivityMonitor monitor;

        StartOrder(Game game) {
            instrumentation = ReflectionHelpers.getField(game, "mInstrumentation");
            monitor = new android.app.Instrumentation.ActivityMonitor() {
                @Override public android.app.Instrumentation.ActivityResult onStartActivity(Intent intent) {
                    finishingAtStart.add(game.isFinishing());
                    return null;
                }
            };
            instrumentation.addMonitor(monitor);
        }

        void close() {
            instrumentation.removeMonitor(monitor);
        }
    }

    static final class TerminationProbe {
        final java.util.concurrent.CountDownLatch ended = new java.util.concurrent.CountDownLatch(1);
        volatile Thread endedOn;
        volatile int ends;
        final com.limelight.nvstream.NvConnectionListener listener =
                (com.limelight.nvstream.NvConnectionListener) java.lang.reflect.Proxy.newProxyInstance(
                        com.limelight.nvstream.NvConnectionListener.class.getClassLoader(),
                        new Class<?>[] {com.limelight.nvstream.NvConnectionListener.class}, (proxy, method, args) -> {
                            if (method.getName().equals("connectionTerminated")) {
                                endedOn = Thread.currentThread();
                                ends++;
                                ended.countDown();
                            }
                            return null;
                        });
    }

    static void setBudget(Game game, int attempts, long startedAt) {
        game.recoveryAttempts = attempts;
        game.recoveryStartedAt = startedAt;
    }

    static Game recoveryGame() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.AppTheme);
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.autoReconnect = true;
        ReflectionHelpers.setField(game, "prefConfig", config);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null) {
            @Override public void stop() {}
        });
        return game;
    }

    static Game establishedGame() {
        Game game = recoveryGame();
        PreferenceConfiguration config = ReflectionHelpers.getField(game, "prefConfig");
        config.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        config.enableLatencyToast = false;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setField(game, "decoderRenderer", new MediaCodecDecoderRenderer(game, config,
                null, 0, false, false, "", null) {
            @Override public void prepareForStop() {}
        });
        // The stream showed video before it dropped.
        ReflectionHelpers.setField(ReflectionHelpers.getField(game, "decoderRenderer"), "lastVideoFrameTimeMs", 1L);
        ReflectionHelpers.setField(game, "tombstonePrefs", game.getSharedPreferences("recovery-test", 0));
        ReflectionHelpers.setField(game, "attemptedConnection", true);
        ReflectionHelpers.setField(game, "recoveryAppId", 73);
        ReflectionHelpers.setField(game, "recoveryAppUuid", "captured-game");
        ReflectionHelpers.setField(game.streamRecovery, "connectedAt", SystemClock.elapsedRealtime());
        return game;
    }

    static void assertRestoredMute(Intent retry, boolean muted) {
        assertNotNull(retry);
        assertEquals(muted, retry.getBooleanExtra(MICROPHONE_MUTED, false));
        Game replacement = Robolectric.buildActivity(Game.class).get();
        replacement.setIntent(retry);
        MuteConnection connection = new MuteConnection();
        ReflectionHelpers.setField(replacement, "conn", connection);
        ReflectionHelpers.callInstanceMethod(replacement.streamRecovery, "restoreMicrophoneMute");
        assertEquals(muted, connection.isMicrophoneMuted());
        assertEquals(muted, connection.isMicrophoneMutedForReconnect());
    }

    static void awaitHostAction(Game game) throws InterruptedException {
        long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MILLISECONDS.toNanos(HANG_GUARD_MS);
        while (Boolean.TRUE.equals(ReflectionHelpers.getField(game, "hostActionInProgress"))
                && System.nanoTime() < deadline) {
            Thread.sleep(5);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
        }
        assertFalse(ReflectionHelpers.getField(game, "hostActionInProgress"));
    }

    /** A connection on a PC that says the stream moved to {@code device}, and notes whether a recovery was to follow. */
    static MuteConnection movedTo(String device, java.util.List<Boolean> asked) {
        return new MuteConnection() {
            @Override public boolean offersContinueHere() { return true; }
            @Override public String streamMovedTo(boolean recoveryFollows) {
                asked.add(recoveryFollows);
                return recoveryFollows ? null : device;
            }
        };
    }

    /** Runs the UI thread until {@code done}: what runs off it answers on its own time. */
    static void awaitOnUi(java.util.function.BooleanSupplier done) throws InterruptedException {
        long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MILLISECONDS.toNanos(HANG_GUARD_MS);
        while (!done.getAsBoolean() && System.nanoTime() < deadline) {
            Thread.sleep(5);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
        }
        assertTrue(done.getAsBoolean());
    }

    /** A PC that gives these answers to its reads, the last one for good; null is no answer at all. */
    static com.limelight.nativeapi.NativeApiClient pcAnswering(java.util.concurrent.atomic.AtomicInteger reads, String... answers) {
        return new com.limelight.nativeapi.NativeApiClient((host, op, argument) -> {
            String answer = answers[Math.min(reads.getAndIncrement(), answers.length - 1)];
            if (answer == null) {
                throw new com.limelight.nativeapi.NativeApiException(
                        com.limelight.nativeapi.NativeApiException.Kind.NETWORK, "refused");
            }
            return "{\"ok\":" + answer + "}";
        }, new com.limelight.nativeapi.HostEndpoint("pc", "192.0.2.1", 47989, 47984, "pem", "device", new byte[0], new byte[0]));
    }

    /** A stream that ended on the move to the TV, with its prompt open and watching {@code pc}. */
    static Game movedAwayFrom(com.limelight.nativeapi.NativeApiClient pc) {
        Game game = establishedGame();
        PreferenceConfiguration prefs = ReflectionHelpers.getField(game, "prefConfig");
        prefs.autoReconnect = false;
        ReflectionHelpers.setField(game, "conn", new MuteConnection() {
            @Override public String streamMovedTo(boolean recoveryFollows) { return "TV"; }
            @Override public com.limelight.nativeapi.NativeApiClient nativeApiClient() { return pc; }
        });
        game.connectionTerminated(-1);
        return game;
    }

    static void passWatchInterval() {
        org.robolectric.shadows.ShadowLooper.idleMainLooper(3, java.util.concurrent.TimeUnit.SECONDS);
    }

    /** The watch's interval passes until the PC was read {@code count} times and the UI thread heard the last answer. */
    static void awaitReads(java.util.concurrent.atomic.AtomicInteger reads, int count) throws InterruptedException {
        awaitOnUi(() -> {
            passWatchInterval();
            return reads.get() >= count;
        });
        // The answer is on its way to the UI thread.
        for (int i = 0; i < 20; i++) {
            Thread.sleep(5);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
        }
    }

    static String promptTitle(Game game) {
        android.app.Dialog prompt = ReflectionHelpers.getField(game, "streamMenu");
        assertNotNull(prompt);
        android.widget.TextView text = prompt.findViewById(androidx.appcompat.R.id.alertTitle);
        return text.getText().toString();
    }

    static android.widget.Button promptButton(Game game, int which) {
        androidx.appcompat.app.AlertDialog prompt = ReflectionHelpers.getField(game, "streamMenu");
        return prompt.getButton(which);
    }

    static String promptMessage(Game game) {
        android.app.Dialog prompt = ReflectionHelpers.getField(game, "streamMenu");
        if (prompt == null) return null;
        android.widget.TextView text = prompt.findViewById(android.R.id.message);
        return text == null ? null : text.getText().toString();
    }

    static void assertMovedPrompt(Game game, String title) {
        assertFalse("the moved stream stays on its prompt", game.isFinishing());
        assertTrue(ReflectionHelpers.getField(game, "displayedFailureDialog"));
        assertNull(ReflectionHelpers.getField(game, "pendingRecovery"));
        android.app.Dialog prompt = ReflectionHelpers.getField(game, "streamMenu");
        assertNotNull(prompt);
        android.widget.TextView text = prompt.findViewById(androidx.appcompat.R.id.alertTitle);
        assertEquals(title, text.getText().toString());
        assertNull("nothing reconnects", org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
    }

    /** A start that joins the game the PC runs, whoever owns it. */
    static void assertJoins(Intent started) {
        assertEquals(Game.class.getName(), started.getComponent().getClassName());
        assertTrue("joins", started.getBooleanExtra(GameRecovery.EXTRA_JOIN_RUNNING, false));
        assertFalse(started.getBooleanExtra("rubylight.outputResumeOnly", false));
        assertEquals(73, started.getIntExtra(Game.EXTRA_APP_ID, 0));
    }

    /** A start that joins only a game of this device's own, and fails otherwise. */
    static void assertResumesOnly(Intent started) {
        assertEquals(Game.class.getName(), started.getComponent().getClassName());
        assertTrue(started.getBooleanExtra("rubylight.outputResumeOnly", false));
        assertFalse("does not join", started.getBooleanExtra(GameRecovery.EXTRA_JOIN_RUNNING, false));
    }

    static void placeWindow(Game game, boolean taskRoot, boolean multiWindow, boolean pictureInPicture) {
        org.robolectric.shadows.ShadowActivity window = org.robolectric.Shadows.shadowOf(game);
        window.setIsTaskRoot(taskRoot);
        window.setInMultiWindowMode(multiWindow);
        ReflectionHelpers.setField(window, "isInPictureInPictureMode", pictureInPicture);
    }

    /** In place, the next stream clears this stream's task (NEW_TASK with CLEAR_TASK) and starts before this one finishes. */
    static void assertHandOff(String condition, Game game, StartOrder order, Intent started, boolean inPlace) {
        assertEquals(condition, Game.class.getName(), started.getComponent().getClassName());
        assertEquals(condition, inPlace, (started.getFlags() & Intent.FLAG_ACTIVITY_CLEAR_TASK) != 0);
        assertEquals(condition, inPlace, (started.getFlags() & Intent.FLAG_ACTIVITY_NEW_TASK) != 0);
        assertEquals(condition + ": finishing when the next one started",
                java.util.Collections.singletonList(!inPlace), order.finishingAtStart);
        assertTrue(condition, game.isFinishing());
    }

    /** The replacement activity starts with {@code intent}: the budget onCreate reads. */
    static void startStream(Game stream, Intent intent) {
        stream.setIntent(intent);
        stream.streamRecovery.readRecoveryBudget(SystemClock.elapsedRealtime());
    }

    /**
     * Leaves for {@code awayMs} (Files, Home) and returns: the resume runs at once and, in the
     * replacement activity, connects with the budget the stream had when it was left.
     */
    static Game leaveAndReturn(Game stream, long awayMs) throws InterruptedException {
        long leftAt = SystemClock.elapsedRealtime();
        Intent retry = leaveAndComeBack(stream, awayMs);
        int beforeAttempts = stream.recoveryAttempts;
        long beforeStartedAt = stream.recoveryStartedAt;
        Game resumed = establishedGame();
        long createdAt = SystemClock.elapsedRealtime();
        startStream(resumed, retry);
        assertTrue(ReflectionHelpers.callInstanceMethod(resumed.streamRecovery, "isResumeOnlyStream"));
        assertEquals("the trip spent none of the failure budget", beforeAttempts, resumed.recoveryAttempts);
        assertEquals(beforeAttempts == 0 ? 0 : beforeStartedAt + (createdAt - leftAt), resumed.recoveryStartedAt);
        resumed.streamRecovery.onStreamConnected(SystemClock.elapsedRealtime());
        return resumed;
    }

    /** Leaves {@code stream} for {@code awayMs} and returns; the resume's intent, which spent nothing. */
    static Intent leaveAndComeBack(Game stream, long awayMs) throws InterruptedException {
        stream.onPause();
        stream.onStop();
        int beforeAttempts = stream.recoveryAttempts;
        long beforeStartedAt = stream.recoveryStartedAt;
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofMillis(awayMs));
        long returnedAt = SystemClock.elapsedRealtime();
        stream.onResume();
        assertNotNull("the return was refused", ReflectionHelpers.getField(stream, "pendingRecovery"));
        // Its fixed one-second delay, whatever the failure budget holds.
        Intent retry = awaitRecoveryLaunch(stream, 1);
        assertEquals("the resume took an attempt", beforeAttempts, stream.recoveryAttempts);
        assertEquals(beforeStartedAt, stream.recoveryStartedAt);
        cleanupRecovery(stream);
        assertEquals(returnedAt, retry.getLongExtra("rubylight.tripResumeStarted", 0));
        return retry;
    }

    static void assertKeepsStreaming(Game game, String condition) throws InterruptedException {
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        assertFalse(condition, ReflectionHelpers.<Boolean>getField(game, "displayedFailureDialog"));
        assertNull(condition, ReflectionHelpers.getField(game, "pendingRecovery"));
        assertFalse(condition, ReflectionHelpers.<Boolean>getField(game, "recoveryHandoffInProgress"));
        settleHandoff(game);
        assertNull(condition, org.robolectric.Shadows.shadowOf(game).getNextStartedActivity());
    }

    static String noticeAfterLoss(Game game, android.widget.TextView overlay) {
        game.streamNetworkLost();
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        String text = overlay.getText().toString();
        game.streamNetworkReturned(MoonBridge.NETWORK_ROAMED, false);
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
        return text;
    }

    static Intent awaitRecoveryLaunch(Game game, int delaySeconds) throws InterruptedException {
        join(ReflectionHelpers.getField(game, "connectionStopThread"));
        org.robolectric.shadows.ShadowLooper.idleMainLooper(delaySeconds, java.util.concurrent.TimeUnit.SECONDS);
        return awaitStartedActivity(game);
    }

    /** The activity the hand-off started, once its workers have posted it; only a hang fails. */
    static Intent awaitStartedActivity(Game game) throws InterruptedException {
        long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MILLISECONDS.toNanos(HANG_GUARD_MS);
        while (true) {
            settleHandoff(game);
            Intent retry = org.robolectric.Shadows.shadowOf(game).getNextStartedActivity();
            if (retry != null) return retry;
            if (System.nanoTime() > deadline) fail("Recovery did not create the replacement activity");
            Thread.sleep(5);
        }
    }

    /**
     * Joins the stream's stop and the hand-off workers that wait for it, so whatever they post to
     * the UI thread is queued, then runs it: a late continuation shows here, not after the test.
     */
    static void settleHandoff(Game game) throws InterruptedException {
        join(ReflectionHelpers.getField(game, "connectionStopThread"));
        for (Thread thread : Thread.getAllStackTraces().keySet()) {
            if (HANDOFF_WORKERS.contains(thread.getName())) join(thread);
        }
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
    }

    static void join(Thread thread) throws InterruptedException {
        if (thread == null) return;
        thread.join(HANG_GUARD_MS);
        assertFalse(thread.getName() + " did not finish", thread.isAlive());
    }

    static void cleanupRecovery(Game game) {
        ReflectionHelpers.callInstanceMethod(game.streamRecovery, "cancelAutomaticRecovery");
        android.app.Dialog dialog = ReflectionHelpers.getField(game, "streamMenu");
        if (dialog != null) dialog.dismiss();
        com.limelight.utils.SpinnerDialog.closeDialogs(game);
    }

    // The RTSP handshake failed because full chroma cannot be streamed with this PC (code -106).
    static void refuseFullChromaAtStart(Game game) {
        ReflectionHelpers.setField(game, "connected", false);
        ReflectionHelpers.setField(game, "connecting", true);
        game.stageFailed("RTSP handshake", MoonBridge.ML_PORT_FLAG_TCP_48010,
                MoonBridge.CLIENT_ERROR_PACKED_444_REFUSED);
    }
}
