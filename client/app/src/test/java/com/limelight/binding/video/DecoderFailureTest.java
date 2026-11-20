package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.PcViewLifecycleTest;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Proxy;
import java.time.Duration;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** A decoder that keeps failing ends the stream with its reason, never by crashing the app. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, instrumentedPackages = "com.limelight.nvstream.jni",
        shadows = {DecoderFailureTest.FailingCodec.class, PcViewLifecycleTest.NoNativeMoonBridge.class})
public class DecoderFailureTest {
    private static final String RELEASED = "Invalid to call at Released state; only valid in executing state";

    private static final String HEVC = "video/hevc";
    private static final String MTK_HEVC = "c2.mtk.hevc.decoder";

    private final List<Integer> terminated = new CopyOnWriteArrayList<>();
    private final List<Throwable> uncaught = new CopyOnWriteArrayList<>();
    private final List<Exception> crashes = new CopyOnWriteArrayList<>();
    // The stream log's lines (LimeLog) while a test runs.
    private final List<String> log = new CopyOnWriteArrayList<>();
    private final java.util.logging.Handler logLines = new java.util.logging.Handler() {
        @Override public void publish(java.util.logging.LogRecord record) {
            log.add(record.getMessage());
        }
        @Override public void flush() {}
        @Override public void close() {}
    };
    private Thread.UncaughtExceptionHandler previousHandler;
    private ActivityController<Activity> activity;
    private MediaCodecDecoderRenderer renderer;
    private MediaCodec codec;

    @Before
    public void setUp() throws Exception {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        java.util.logging.Logger.getLogger(com.limelight.LimeLog.class.getName()).addHandler(logLines);
        previousHandler = Thread.getDefaultUncaughtExceptionHandler();
        Thread.setDefaultUncaughtExceptionHandler((thread, e) -> uncaught.add(e));
        NvConnectionListener listener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("connectionTerminated")) terminated.add((Integer) args[0]);
                    return null;
                });
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", listener);

        activity = Robolectric.buildActivity(Activity.class).create();
        renderer = new MediaCodecDecoderRenderer(activity.get(), new PreferenceConfiguration(), crashes::add, 0,
                false, false, "", null);
        FailingCodec.released = true;
        codec = MediaCodec.createDecoderByType("video/hevc");
        ReflectionHelpers.setField(renderer, "videoDecoder", codec);
        ReflectionHelpers.setField(renderer, "activeDecoderName", "c2.mtk.hevc.decoder");
        ReflectionHelpers.setField(renderer, "initialWidth", 3840);
        ReflectionHelpers.setField(renderer, "initialHeight", 2160);
        // Each reset "works" and the codec fails again at once, as the TV's out of memory did.
        ReflectionHelpers.setField(renderer, "recoveryCodec", new NoOpRecovery());
        ReflectionHelpers.setField(renderer, "recoveryBackoff", new DecoderRecoveryBackoff(new long[] {0, 0, 0, 0, 0}));
    }

    @After
    public void tearDown() throws Exception {
        ReflectionHelpers.setField(renderer, "stopping", true);
        Thread thread = ReflectionHelpers.getField(renderer, "rendererThread");
        if (thread != null) thread.join(5000);
        ReflectionHelpers.setStaticField(MoonBridge.class, "connectionListener", null);
        Thread.setDefaultUncaughtExceptionHandler(previousHandler);
        FailingCodec.released = true;
        // A decoder a test set up through setup's ladder (streamWithVendorKeys).
        MediaCodec own = ReflectionHelpers.getField(renderer, "videoDecoder");
        if (own != null && own != codec) own.release();
        codec.release();
        activity.get().getSharedPreferences(VendorKeyHangs.PREFS, 0).edit().clear().commit();
        activity.close();
        java.util.logging.Logger.getLogger(com.limelight.LimeLog.class.getName()).removeHandler(logLines);
    }

    @Test
    public void aReleasedCodecEndsTheStreamInsteadOfCrashingTheRendererThread() throws Exception {
        ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
        Thread rendererThread = ReflectionHelpers.getField(renderer, "rendererThread");

        // The input thread: every dequeue throws, as in the TV's crash (fetchNextInputBuffer). It
        // runs beside the test, so a recovery that never ends fails the test instead of hanging it.
        List<Boolean> fetched = new CopyOnWriteArrayList<>();
        Thread input = new Thread(() -> {
            for (int i = 0; i < 200 && !(boolean) ReflectionHelpers.getField(renderer, "stopping"); i++) {
                fetched.add(ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
            }
        });
        input.setDaemon(true);
        input.start();
        try {
            input.join(30_000);
            rendererThread.join(5000);
            assertFalse("Input thread still running", input.isAlive());
        } finally {
            renderer.prepareForStop();
        }

        assertFalse(fetched.toString(), fetched.contains(true));
        assertFalse("Renderer thread still running", rendererThread.isAlive());
        assertEquals(Collections.emptyList(), uncaught);
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        String failure = renderer.getDecoderFailure();
        assertNotNull(failure);
        assertTrue(failure, failure.startsWith("c2.mtk.hevc.decoder failed again after 5 recoveries in a row" +
                " without a frame at 3840x2160 (out of memory"));
        assertFalse(renderer.decoderFailedWithVulkan());
        // Five recoveries, not ten in 2 s: the sixth gives up.
        assertEquals(5, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        // Reported once, so the next stream runs without reference frame invalidation.
        assertEquals(1, crashes.size());
        assertTrue(crashes.get(0) instanceof MediaCodecDecoderRenderer.RendererException);
        // Logs and crash reports read the message and cause: the diagnostics are in the message,
        // and the codec's own exception stays the cause.
        Exception crash = crashes.get(0);
        assertEquals(crash.toString(), crash.getMessage());
        assertTrue(crash.getMessage(), crash.getMessage().contains("Format: "));
        assertTrue(crash.getMessage(), crash.getMessage().contains("Consecutive crashes: "));
        assertTrue(String.valueOf(crash.getCause()), crash.getCause() instanceof IllegalStateException);
    }

    @Test
    public void stoppingDuringABackOffWakesTheWaitingThreadsAtOnce() throws Exception {
        // The second wait is far longer than any stop may take, so the test holds on a slow runner:
        // with 250 ms, a busy CI machine saw the third recovery begin before it looked.
        DecoderRecoveryBackoff backoff = new DecoderRecoveryBackoff(new long[] {0, 60_000, 60_000, 60_000, 60_000});
        ReflectionHelpers.setField(renderer, "recoveryBackoff", backoff);
        ReflectionHelpers.callInstanceMethod(renderer, "startRendererThread");
        Thread rendererThread = ReflectionHelpers.getField(renderer, "rendererThread");
        Thread input = new Thread(() -> {
            while (!(boolean) ReflectionHelpers.getField(renderer, "stopping")) {
                ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer");
            }
        });
        // A failed assertion must not leave a thread that keeps the test JVM from exiting.
        input.setDaemon(true);
        input.start();
        long tookMs;
        try {
            // The second recovery in a row without a frame has begun its wait.
            long deadline = System.nanoTime() + 30_000_000_000L;
            while (backoff.recoveriesWithoutFrame() < 2 && System.nanoTime() < deadline) Thread.sleep(1);
            assertEquals(2, backoff.recoveriesWithoutFrame());

            // The stream ends from another thread, as the UI's stop does: the renderer thread is
            // interrupted and the input thread woken, instead of either sleeping the wait out.
            long stopNs = System.nanoTime();
            Thread stopper = new Thread(renderer::prepareForStop);
            stopper.start();
            stopper.join(5000);
            input.join(5000);
            rendererThread.join(5000);
            tookMs = (System.nanoTime() - stopNs) / 1_000_000;
        } finally {
            // Wakes both threads if an assertion above failed before the stop.
            renderer.prepareForStop();
            input.join(5000);
        }

        assertFalse("Input thread still running", input.isAlive());
        assertFalse("Renderer thread still running", rendererThread.isAlive());
        // Woken at once: well under the 60 s wait either thread was in.
        assertTrue("Stopping took " + tookMs + " ms", tookMs < 2000);
        assertEquals(Collections.emptyList(), uncaught);
        // A stop is not a decoder failure: no -107 and no reason.
        assertEquals(Collections.emptyList(), terminated);
        assertNull(renderer.getDecoderFailure());
        assertEquals(2, backoff.recoveriesWithoutFrame());
    }

    @Test
    public void aThreadWaitingForARecoveryLeavesOnceItIsDoneEvenIfTheDecoderFailedAgain() throws Exception {
        // The renderer thread has quiesced and waits for the input thread to recover the codec.
        failOnce();
        int renderThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD");
        int inputThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_INPUT_THREAD");
        Thread waiting = new Thread(() -> ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                ReflectionHelpers.ClassParameter.from(int.class, renderThread)));
        waiting.setDaemon(true);
        waiting.start();
        Object monitor = ReflectionHelpers.getField(renderer, "codecRecoveryMonitor");
        long deadline = System.nanoTime() + 30_000_000_000L;
        while (waiting.getState() != Thread.State.TIMED_WAITING && System.nanoTime() < deadline) Thread.sleep(1);
        try {
            assertEquals(Thread.State.TIMED_WAITING, waiting.getState());
            // The input thread recovers, and its next dequeue throws before the waiting thread
            // looked again (a busy CPU): a new recovery is due. Holding the monitor makes that order
            // certain. The waiting thread kept waiting for this one, which needs it to quiesce
            // first, and the input thread waited for it: both decoder threads hung for good.
            synchronized (monitor) {
                assertTrue((boolean) ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                        ReflectionHelpers.ClassParameter.from(int.class, inputThread)));
                failOnce();
            }
            waiting.join(5000);
            assertFalse("The waiting thread never left its finished recovery", waiting.isAlive());
            assertEquals(1, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));

            // Both threads quiesce again, and the next recovery runs.
            recoverOnTheInputThread();
            assertEquals(2, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        } finally {
            renderer.prepareForStop();
            waiting.join(5000);
        }
    }

    @Test
    public void plannedRestartsNeitherWaitNorCountTowardGivingUp() {
        // Six planned restarts (HDR changes) with no frame between them: a healthy stream, no wait.
        DecoderRecoveryBackoff backoff = new DecoderRecoveryBackoff(new long[] {0, 60_000, 60_000, 60_000, 60_000});
        ReflectionHelpers.setField(renderer, "recoveryBackoff", backoff);
        for (int i = 0; i < 6; i++) {
            ReflectionHelpers.callInstanceMethod(renderer, "requestRestart",
                    ReflectionHelpers.ClassParameter.from(String.class, "HDR on"));
            recoverOnTheInputThread();
        }
        assertEquals(0, backoff.recoveriesWithoutFrame());
        assertEquals(6, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        assertTrue(terminated.isEmpty());
        assertNull(renderer.getDecoderFailure());
    }

    @Test
    public void aDecoderThatRefusesItsConfigurationOnAValidSurfaceEndsTheStream() {
        SurfaceTexture texture = new SurfaceTexture(0);
        Surface surface = new Surface(texture);
        try {
            ReflectionHelpers.setField(renderer, "renderTarget", surface);
            ReflectionHelpers.setField(renderer, "recoveryCodec", new RefusingRecovery());
            failOnce();
            recoverOnTheInputThread();
            assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
            assertEquals("c2.mtk.hevc.decoder refused its configuration after a recovery", renderer.getDecoderFailure());

            // With the surface gone, the same refusal only stops: the stream is ending anyway.
            terminated.clear();
            ReflectionHelpers.setField(renderer, "decoderFailure", null);
            ReflectionHelpers.setField(renderer, "stopping", false);
            surface.release();
            failOnce();
            recoverOnTheInputThread();
            assertTrue(terminated.isEmpty());
            assertTrue((boolean) ReflectionHelpers.getField(renderer, "stopping"));
        } finally {
            surface.release();
            texture.release();
        }
    }

    @Test
    public void anExceptionIsForgivenOnceFramesFlowAgain() {
        ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
        failOnce();
        // Frames come out again: an exception a minute later is a new first one, not the end.
        ReflectionHelpers.setField(renderer, "numFramesOut", 30);
        ShadowSystemClock.advanceBy(Duration.ofMillis(60_000));
        failOnce();
        assertTrue(terminated.isEmpty());

        // And once frames flow after that one too, the hang check is back on.
        ReflectionHelpers.setField(renderer, "numFramesOut", 31);
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(),
                renderer.getDecoderFailure().startsWith("c2.mtk.hevc.decoder took no input for 5000 ms"));
    }

    @Test
    public void aDecoderThatStopsTakingInputAfterAnEatenExceptionStillEndsTheStream() {
        // One exception eaten (as at teardown), then no frame and no input without another throw:
        // the hang check stayed off for good.
        ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
        failOnce();
        assertTrue(terminated.isEmpty());
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(),
                renderer.getDecoderFailure().startsWith("c2.mtk.hevc.decoder took no input for 5000 ms"));
    }

    private void failOnce() {
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, new IllegalStateException(RELEASED)));
    }

    // The renderer thread has quiesced; the input thread is the last and runs the recovery.
    private void recoverOnTheInputThread() {
        int renderThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_RENDER_THREAD");
        int inputThread = ReflectionHelpers.getStaticField(MediaCodecDecoderRenderer.class, "CR_FLAG_INPUT_THREAD");
        ReflectionHelpers.setField(renderer, "codecRecoveryThreadQuiescedFlags", renderThread);
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(renderer, "doCodecRecoveryIfRequired",
                ReflectionHelpers.ClassParameter.from(int.class, inputThread)));
    }

    @Test
    public void exceptionsAfterTheLastRecoveryEndTheStreamInsteadOfThrowing() {
        ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
        IllegalStateException released = new IllegalStateException(RELEASED);
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, released));
        assertTrue(terminated.isEmpty());

        // Still failing 3 s later: this threw from the renderer thread and crashed the app.
        ShadowSystemClock.advanceBy(Duration.ofMillis(3000));
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, released));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(), renderer.getDecoderFailure().contains(RELEASED));
        assertTrue((boolean) ReflectionHelpers.getField(renderer, "stopping"));

        // Later exceptions are eaten, and the stream is ended once.
        ReflectionHelpers.callInstanceMethod(renderer, "handleDecoderException",
                ReflectionHelpers.ClassParameter.from(IllegalStateException.class, released));
        assertEquals(1, terminated.size());
    }

    @Test
    public void aDecoderThatTakesNoInputIsResetOnceThenEndsTheStream() {
        // The TV's HDR10 stream: 5000 ms dequeues, and the engine dropped the exception thrown for
        // it, so the picture stayed frozen. It ended the stream with no reset at all.
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertTrue(terminated.isEmpty());
        assertEquals(3, ((AtomicInteger)
                ReflectionHelpers.getField(renderer, "codecRecoveryType")).get());
        recoverOnTheInputThread();
        assertEquals(1, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        assertFalse((boolean) ReflectionHelpers.getField(renderer, "hangResetPending"));

        // Still no input and no frame after the reset: the stream ends.
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(), renderer.getDecoderFailure().startsWith(
                "c2.mtk.hevc.decoder took no input for 5000 ms, also after a reset"));
    }

    @Test
    public void aDecoderThatHangsAgainAfterFramesFlowedIsResetAgain() {
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        recoverOnTheInputThread();
        ReflectionHelpers.setField(renderer, "numFramesOut", 1);
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        recoverOnTheInputThread();
        assertEquals(2, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        assertTrue(terminated.isEmpty());

        // With the attempts used up, a hang ends the stream without a reset.
        ReflectionHelpers.setField(renderer, "numFramesOut", 2);
        ReflectionHelpers.setField(renderer, "codecRecoveryAttempts", 10);
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertEquals("c2.mtk.hevc.decoder took no input for 5000 ms", renderer.getDecoderFailure());
    }

    @Test
    public void aResetOfAHungDecoderThatHangsItselfEndsTheStream() throws Exception {
        ReflectionHelpers.setField(renderer, "hangResetTimeoutMs", 50L);
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        // The thread running the reset hangs inside MediaCodec, holding the recovery monitor.
        Object monitor = ReflectionHelpers.getField(renderer, "codecRecoveryMonitor");
        Thread ui;
        synchronized (monitor) {
            long deadline = System.nanoTime() + 10_000_000_000L;
            while (terminated.isEmpty() && System.nanoTime() < deadline) Thread.sleep(5);
            // Leaving after the -107 dialog, with the reset still hung: the UI's calls into the
            // renderer took the same monitor and waited for good (an ANR).
            ui = new Thread(() -> {
                renderer.setStreamFrameRateMilliHz(120_000);
                assertNull(renderer.getReconfigureLimits());
                assertFalse(renderer.prepareForReconfigure(1920, 1080));
                renderer.setHdrMode(true, null);
                renderer.prepareForStop();
            });
            ui.setDaemon(true);
            ui.start();
            ui.join(5000);
            assertFalse("The UI waited for the hung reset's monitor", ui.isAlive());
        }
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertEquals("c2.mtk.hevc.decoder took no input for 5000 ms, and its reset did not finish within 50 ms",
                renderer.getDecoderFailure());
        assertTrue((boolean) ReflectionHelpers.getField(renderer, "stopping"));
        assertEquals(1, crashes.size());
        assertEquals(Collections.emptyList(), uncaught);
    }

    /**
     * Leaving while a reset runs inside MediaCodec, which an HDR change asks for and a big AV1
     * decoder takes seconds over (#69): the UI's stop waited for the reset's monitor though the
     * decoder had not failed, an ANR. It no longer waits; the shutdown worker's stop wakes the
     * decoder threads.
     */
    @Test
    public void theUiStopDoesNotWaitForAResetStillRunning() throws Exception {
        Object monitor = ReflectionHelpers.getField(renderer, "codecRecoveryMonitor");
        java.util.concurrent.CountDownLatch holding = new java.util.concurrent.CountDownLatch(1);
        Thread reset = new Thread(() -> {
            synchronized (monitor) {
                holding.countDown();
                try {
                    Thread.sleep(3000);
                } catch (InterruptedException ignored) {
                    // The test is done.
                }
            }
        });
        reset.setDaemon(true);
        reset.start();
        holding.await();
        assertEquals("the stop runs on the UI thread", android.os.Looper.getMainLooper(), android.os.Looper.myLooper());
        long startNs = System.nanoTime();
        renderer.prepareForStop();
        long tookMs = (System.nanoTime() - startNs) / 1_000_000;
        reset.interrupt();
        reset.join(5000);
        assertTrue("The UI waited " + tookMs + " ms for the reset's monitor", tookMs < 1000);
        assertTrue((boolean) ReflectionHelpers.getField(renderer, "stopping"));
        assertNull(renderer.getDecoderFailure());
        assertEquals(Collections.emptyList(), terminated);
    }

    @Test
    public void aResetOfAHungDecoderThatFinishesLeavesTheStreamRunning() throws Exception {
        ReflectionHelpers.setField(renderer, "hangResetTimeoutMs", 50L);
        FailingCodec.released = false;
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer"));
        recoverOnTheInputThread();
        Thread.sleep(300);
        assertTrue(terminated.isEmpty());
        assertNull(renderer.getDecoderFailure());
    }

    @Test
    public void recoveryAttemptsRefillAfterGoodFramesButNotInACrashLoop() {
        // Ten recoveries 100 frames apart: a crash loop, the next failure is not recovered.
        for (int i = 0; i < 10; i++) {
            ReflectionHelpers.setField(renderer, "numFramesOut", i * 100);
            failOnce();
            recoverOnTheInputThread();
        }
        assertEquals(10, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        ReflectionHelpers.setField(renderer, "numFramesOut", 900 + 599);
        failOnce();
        assertEquals(0, ((AtomicInteger)
                ReflectionHelpers.getField(renderer, "codecRecoveryType")).get());

        // 600 frames within a few seconds (240 FPS) is still a crash loop.
        ReflectionHelpers.setField(renderer, "numFramesOut", 900 + 600);
        ShadowSystemClock.advanceBy(Duration.ofMillis(9_999));
        failOnce();
        assertEquals(0, ((AtomicInteger) ReflectionHelpers.getField(renderer, "codecRecoveryType")).get());

        // 600 good frames and 10 s since the last recovery: hours later, it is recovered again.
        ShadowSystemClock.advanceBy(Duration.ofMillis(1));
        failOnce();
        assertEquals(3, ((AtomicInteger)
                ReflectionHelpers.getField(renderer, "codecRecoveryType")).get());
        recoverOnTheInputThread();
        assertEquals(1, (int) ReflectionHelpers.getField(renderer, "codecRecoveryAttempts"));
        assertTrue(terminated.isEmpty());
    }

    /**
     * The TV's hang (TCL C6K: no input after one to four frames with the vendor keys, a clean stream
     * without them): the first hang gets the reset with the same keys any decoder gets; hanging again
     * before it ran well, the decoder is set up by setup's ladder without them, and once it ran well
     * so, the next streams start it without them.
     */
    @Test
    public void aDecoderThatHangsWithItsVendorKeysAgainAfterAResetRunsWithoutThem() {
        renderer = streamWithVendorKeys(MTK_HEVC, HEVC);
        RealRebuild recovery = realRebuild();
        List<String> tuned = vendorKeys(ReflectionHelpers.getField(renderer, "configuredFormat"));
        assertFalse("the MediaTek preset", tuned.isEmpty());

        FailingCodec.released = false;
        assertFalse(fetch());
        recoverOnTheInputThread();
        assertEquals(Arrays.asList("reset", "configure"), recovery.steps);

        // Three frames, as the TV put out, and no input again: set up anew without the keys instead
        // of another reset with them.
        ReflectionHelpers.setField(renderer, "numFramesOut", 3);
        assertFalse(fetch());
        recoverOnTheInputThread();
        assertEquals(Arrays.asList("reset", "configure", "rebuild"), recovery.steps);
        assertTrue(terminated.isEmpty());
        assertNull(renderer.getDecoderFailure());
        MediaFormat keyless = ReflectionHelpers.getField(renderer, "configuredFormat");
        assertEquals(Collections.emptyList(), vendorKeys(keyless));
        assertEquals("Android's own flag stays", 1, keyless.getInteger(MediaFormat.KEY_LOW_LATENCY));
        List<String> dropped = ReflectionHelpers.getField(renderer, "lowLatencyKeysHung");
        List<String> droppedNames = new ArrayList<>();
        for (String key : dropped) droppedNames.add(key.substring(0, key.indexOf('=')));
        assertTrue(dropped.toString(), droppedNames.containsAll(tuned));
        // The log says the keys went after a hang, and which.
        assertTrue(log.toString(), log.stream().anyMatch(line -> line.startsWith(MTK_HEVC + " took no input for ") &&
                line.endsWith(" ms with its vendor low-latency keys, again after a reset; setting it up again" +
                        " without them: " + String.join(", ", dropped))));
        assertTrue(log.toString(), log.contains("Decoder " + MTK_HEVC + " gets no vendor low-latency keys: it took" +
                " no input with them (" + String.join(", ", dropped) + ")"));
        assertTrue(log.toString(), log.stream().anyMatch(line -> line.startsWith("Decoder " + MTK_HEVC +
                " (vendor keys not run on this hardware) tuning keys applied: ") &&
                line.endsWith("; dropped after a hang: " + String.join(", ", dropped))));

        // Not recorded before it ran well without them (600 frames over 10 s)...
        ReflectionHelpers.setField(renderer, "numFramesOut", 3 + 599);
        ShadowSystemClock.advanceBy(Duration.ofMillis(10_000));
        ReflectionHelpers.callInstanceMethod(renderer, "recordVendorKeyHang");
        assertNull(VendorKeyHangs.recorded(activity.get(), VendorKeyHangs.key(MTK_HEVC, HEVC)));
        // ...and once it did, for the next streams, with the keys it hung with.
        ReflectionHelpers.setField(renderer, "numFramesOut", 3 + 600);
        ReflectionHelpers.callInstanceMethod(renderer, "recordVendorKeyHang");
        assertEquals(dropped, VendorKeyHangs.recorded(activity.get(), VendorKeyHangs.key(MTK_HEVC, HEVC)));
        assertTrue(terminated.isEmpty());
    }

    @Test
    public void aDecoderThatHangsWithoutItsVendorKeysTooEndsTheStreamUnrecorded() {
        renderer = streamWithVendorKeys(MTK_HEVC, HEVC);
        RealRebuild recovery = realRebuild();
        FailingCodec.released = false;
        assertFalse(fetch());
        recoverOnTheInputThread();
        // No frame since the reset: today this ended the stream; now the keys go first.
        assertFalse(fetch());
        recoverOnTheInputThread();
        assertEquals(Arrays.asList("reset", "configure", "rebuild"), recovery.steps);
        assertTrue(terminated.isEmpty());

        // No frame without them either: the stream ends, with no other try.
        assertFalse(fetch());
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        String failure = renderer.getDecoderFailure();
        assertTrue(failure, failure.startsWith(MTK_HEVC + " took no input for "));
        assertTrue(failure, failure.contains(", also after its setup without vendor low-latency keys"));
        assertEquals(Arrays.asList("reset", "configure", "rebuild"), recovery.steps);
        // The keys were not what hung it: nothing is recorded against them.
        ReflectionHelpers.setField(renderer, "numFramesOut", 600);
        ShadowSystemClock.advanceBy(Duration.ofMillis(10_000));
        ReflectionHelpers.callInstanceMethod(renderer, "recordVendorKeyHang");
        assertNull(VendorKeyHangs.recorded(activity.get(), VendorKeyHangs.key(MTK_HEVC, HEVC)));
    }

    @Test
    public void aDecoderThatAlsoHangsWithoutItsVendorKeysIsNotRecordedThoughItRunsWellLater() {
        renderer = streamWithVendorKeys(MTK_HEVC, HEVC);
        RealRebuild recovery = realRebuild();
        FailingCodec.released = false;
        assertFalse(fetch());
        recoverOnTheInputThread();
        assertFalse(fetch());
        recoverOnTheInputThread();
        // Frames without the keys, then no input again: reset as any decoder, without the keys.
        ReflectionHelpers.setField(renderer, "numFramesOut", 10);
        assertFalse(fetch());
        recoverOnTheInputThread();
        assertEquals(Arrays.asList("reset", "configure", "rebuild", "reset", "configure"), recovery.steps);
        // It runs well after that reset, but the keys were not all of the hang: no record.
        ReflectionHelpers.setField(renderer, "numFramesOut", 10 + 600);
        ShadowSystemClock.advanceBy(Duration.ofMillis(10_000));
        ReflectionHelpers.callInstanceMethod(renderer, "recordVendorKeyHang");
        assertNull(VendorKeyHangs.recorded(activity.get(), VendorKeyHangs.key(MTK_HEVC, HEVC)));
        assertTrue(terminated.isEmpty());
    }

    @Test
    public void aDecoderThatRanWellAfterItsResetIsResetWithItsKeysAgain() {
        renderer = streamWithVendorKeys(MTK_HEVC, HEVC);
        RealRebuild recovery = realRebuild();
        FailingCodec.released = false;
        assertFalse(fetch());
        recoverOnTheInputThread();
        // 600 frames over 10 s since: a later hang is a first one again, and the keys stay.
        ReflectionHelpers.setField(renderer, "numFramesOut", 600);
        ShadowSystemClock.advanceBy(Duration.ofMillis(10_000));
        assertFalse(fetch());
        recoverOnTheInputThread();
        assertEquals(Arrays.asList("reset", "configure", "reset", "configure"), recovery.steps);
        assertNull(ReflectionHelpers.getField(renderer, "vendorKeysHung"));
        assertFalse(((List<?>) ReflectionHelpers.getField(renderer, "lowLatencyKeysVendor")).isEmpty());
        assertTrue(terminated.isEmpty());
    }

    @Test
    public void aDecoderWhoseFormatHasNoVendorKeysEndsTheStreamAsBefore() {
        // The vendor setting is on, but this decoder's format gets nothing from it.
        renderer = streamWithVendorKeys("c2.android.hevc.decoder", HEVC);
        assertEquals(Collections.emptyList(), ReflectionHelpers.getField(renderer, "lowLatencyKeysVendor"));
        RealRebuild recovery = realRebuild();
        FailingCodec.released = false;
        assertFalse(fetch());
        recoverOnTheInputThread();
        assertFalse(fetch());
        assertEquals(Arrays.asList("reset", "configure"), recovery.steps);
        assertEquals(Collections.singletonList(MoonBridge.CLIENT_ERROR_DECODER), terminated);
        assertTrue(renderer.getDecoderFailure(), renderer.getDecoderFailure().startsWith(
                "c2.android.hevc.decoder took no input for 5000 ms, also after a reset"));
    }

    @Test
    public void theNextStreamStartsThatDecoderWithoutItsVendorKeysAndNoOtherOne() {
        List<String> hung = Arrays.asList("vendor.mtk.vdec.low-latency.mode=1", "operating-rate=32767");
        VendorKeyHangs.record(activity.get(), VendorKeyHangs.key(MTK_HEVC, HEVC), hung);

        renderer = streamWithVendorKeys(MTK_HEVC, HEVC);
        assertEquals(Collections.emptyList(), vendorKeys(ReflectionHelpers.getField(renderer, "configuredFormat")));
        // Nothing left to drop: a hang there is reset and ends the stream as for any decoder.
        assertEquals(Collections.emptyList(), ReflectionHelpers.getField(renderer, "lowLatencyKeysVendor"));
        assertTrue(log.toString(), log.contains("Decoder " + MTK_HEVC + " gets no vendor low-latency keys: it took" +
                " no input with them in an earlier stream (" + String.join(", ", hung) +
                "); an app or system update tries them again"));
        releaseDecoder(renderer);

        // Another decoder of the same kind keeps them, as does the same name for another format.
        renderer = streamWithVendorKeys("OMX.MTK.VIDEO.DECODER.HEVC", HEVC);
        assertFalse(vendorKeys(ReflectionHelpers.getField(renderer, "configuredFormat")).isEmpty());
        releaseDecoder(renderer);
        renderer = streamWithVendorKeys(MTK_HEVC, "video/av01");
        assertFalse(vendorKeys(ReflectionHelpers.getField(renderer, "configuredFormat")).isEmpty());
        releaseDecoder(renderer);

        // An app or system update gives them back.
        activity.get().getSharedPreferences(VendorKeyHangs.PREFS, 0).edit()
                .putString(VendorKeyHangs.STAMP, "1|an older build").commit();
        renderer = streamWithVendorKeys(MTK_HEVC, HEVC);
        assertFalse(vendorKeys(ReflectionHelpers.getField(renderer, "configuredFormat")).isEmpty());
        assertNull(VendorKeyHangs.recorded(activity.get(), VendorKeyHangs.key(MTK_HEVC, HEVC)));
    }

    // A stream of this decoder set up by setup's own ladder with the vendor keys on (the setting's
    // default), as a stream starts.
    private MediaCodecDecoderRenderer streamWithVendorKeys(String decoderName, String mime) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.codecLowLatency = prefs.vendorLowLatency = prefs.codecPerformance = true;
        MediaCodecDecoderRenderer stream = new MediaCodecDecoderRenderer(activity.get(), prefs, crashes::add, 0,
                false, false, "", null);
        boolean av1 = "video/av01".equals(mime);
        ReflectionHelpers.setField(stream, "videoFormat", av1 ? MoonBridge.VIDEO_FORMAT_AV1_MAIN8 : MoonBridge.VIDEO_FORMAT_H265);
        ReflectionHelpers.setField(stream, av1 ? "av1Decoder" : "hevcDecoder", decoder(decoderName, mime));
        ReflectionHelpers.setField(stream, "initialWidth", 3840);
        ReflectionHelpers.setField(stream, "initialHeight", 2160);
        ReflectionHelpers.setField(stream, "refreshRate", 60);
        ReflectionHelpers.setField(stream, "recoveryBackoff", new DecoderRecoveryBackoff(new long[] {0, 0, 0, 0, 0}));
        assertEquals(0, stream.initializeDecoder(true));
        return stream;
    }

    private static void releaseDecoder(MediaCodecDecoderRenderer stream) {
        MediaCodec decoder = ReflectionHelpers.getField(stream, "videoDecoder");
        decoder.release();
        ReflectionHelpers.setField(stream, "videoDecoder", null);
    }

    private static MediaCodecInfo decoder(String name, String mime) {
        MediaFormat capabilities = MediaFormat.createVideoFormat(mime, 3840, 2160);
        return MediaCodecInfoBuilder.newBuilder().setName(name).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }

    // The vendor keys a configured format carries.
    private static List<String> vendorKeys(MediaFormat format) {
        List<String> keys = new ArrayList<>();
        for (String key : format.getKeys()) {
            if (key.startsWith("vendor.") || key.equals("vdec-lowlatency")) keys.add(key);
        }
        return keys;
    }

    private boolean fetch() {
        return ReflectionHelpers.callInstanceMethod(renderer, "fetchNextInputBuffer");
    }

    // Records the steps of each recovery; a rebuild runs the renderer's own, setup's ladder on a new codec.
    private RealRebuild realRebuild() {
        RealRebuild recovery = new RealRebuild(ReflectionHelpers.getField(renderer, "recoveryCodec"));
        ReflectionHelpers.setField(renderer, "recoveryCodec", recovery);
        return recovery;
    }

    private static final class RealRebuild implements CodecRecoverySteps.Codec {
        final List<String> steps = new ArrayList<>();
        private final CodecRecoverySteps.Codec real;

        RealRebuild(CodecRecoverySteps.Codec real) {
            this.real = real;
        }

        @Override public void stop() { steps.add("stop"); }
        @Override public boolean recreate() { steps.add("recreate"); return true; }
        @Override public void reset() { steps.add("reset"); }
        @Override public void configure() { steps.add("configure"); }
        @Override public void rebuild(boolean codecFailed) {
            steps.add("rebuild");
            real.rebuild(codecFailed);
        }
    }

    private static final class NoOpRecovery implements CodecRecoverySteps.Codec {
        final List<String> steps = new ArrayList<>();

        @Override public void stop() { steps.add("stop"); }
        @Override public boolean recreate() { steps.add("recreate"); return true; }
        @Override public void reset() { steps.add("reset"); }
        @Override public void configure() { steps.add("configure"); }
        @Override public void rebuild(boolean codecFailed) { steps.add("rebuild"); }
    }

    /** Every configure fails as configure() does for a surface it cannot use. */
    private static final class RefusingRecovery implements CodecRecoverySteps.Codec {
        @Override public void stop() {}
        @Override public boolean recreate() { return true; }
        @Override public void reset() {}
        @Override public void configure() { throw new IllegalArgumentException("configure refused"); }
        @Override public void rebuild(boolean codecFailed) { throw new IllegalArgumentException("configure refused"); }
    }

    /** Released: every call throws, like a codec the platform let go. Otherwise it never has input. */
    @Implements(MediaCodec.class)
    public static class FailingCodec extends ShadowMediaCodec {
        static volatile boolean released;

        @Implementation public int dequeueInputBuffer(long timeoutUs) {
            if (released) throw new IllegalStateException(RELEASED);
            ShadowSystemClock.advanceBy(Duration.ofMillis(1000));
            return MediaCodec.INFO_TRY_AGAIN_LATER;
        }

        @Implementation public int dequeueOutputBuffer(MediaCodec.BufferInfo info, long timeoutUs) {
            if (released) throw new IllegalStateException(RELEASED);
            return MediaCodec.INFO_TRY_AGAIN_LATER;
        }
    }
}
