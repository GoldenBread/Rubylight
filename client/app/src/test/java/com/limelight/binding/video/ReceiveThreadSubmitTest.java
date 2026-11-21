package com.limelight.binding.video;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCrypto;
import android.media.MediaFormat;
import android.view.Surface;

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
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.nio.ByteBuffer;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * With direct submit a frame is submitted on the thread that reads the video socket. A decoder with
 * no free input buffer must not hold it: the call returns within the receive bound, the worker
 * thread does the waiting, and the hang detection still ends a really hung decoder (there).
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {ReceiveThreadSubmitTest.StallingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ReceiveThreadSubmitTest {
    static final String WORKER = "InputSubmit";

    /** A decoder whose input buffers are busy until a test frees them. */
    @Implements(MediaCodec.class)
    public static class StallingCodec extends ShadowMediaCodec {
        static volatile boolean free;
        static final AtomicInteger receiveThreadTries = new AtomicInteger();
        static final List<Integer> queuedSizes = new CopyOnWriteArrayList<>();
        static final List<String> queueThreads = new CopyOnWriteArrayList<>();

        @Implementation public void configure(MediaFormat format, Surface surface, MediaCrypto crypto, int flags) {}
        @Implementation public void start() {}
        @Implementation public int dequeueInputBuffer(long timeoutUs) {
            if (free) return 0;
            if (Thread.currentThread().getName().equals(WORKER)) {
                // Each try takes a second of the simulated clock: the 5 s hang limit passes in a few.
                ShadowSystemClock.advanceBy(Duration.ofMillis(1000));
            } else {
                receiveThreadTries.incrementAndGet();
                ShadowSystemClock.advanceBy(Duration.ofMillis(timeoutUs / 1000));
            }
            return MediaCodec.INFO_TRY_AGAIN_LATER;
        }
        @Implementation public ByteBuffer getInputBuffer(int index) {
            return ByteBuffer.allocate(4096);
        }
        @Implementation public MediaCodecInfo getCodecInfo() {
            return null;
        }
        @Implementation public void queueInputBuffer(int index, int offset, int size, long timeUs, int flags) {
            queuedSizes.add(size);
            queueThreads.add(Thread.currentThread().getName());
        }
    }

    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;
    private int frame;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        StallingCodec.free = false;
        StallingCodec.receiveThreadTries.set(0);
        StallingCodec.queuedSizes.clear();
        StallingCodec.queueThreads.clear();
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        frame = 1;

        ShadowBuild.setModel("Pixel 8");
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.bitrate = 20_000;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        ReflectionHelpers.setField(renderer, "streamFrameRateMilliHz", 60_000);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265);
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName("c2.android.hevc.decoder").build();
        ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                ClassParameter.from(MediaCodecInfo.class, info),
                ClassParameter.from(MediaFormat.class, format),
                ClassParameter.from(boolean.class, true));
        ReflectionHelpers.setField(renderer, "inputSpacer", null);
        // A P-frame: the first one a stream sees needs no parameter sets here.
        ReflectionHelpers.setField(renderer, "lastFrameNumber", 1);
        renderer.enableSubmitHandoff();
    }

    @After
    public void tearDown() {
        StallingCodec.free = true;
        renderer.prepareForStop();
        renderer.cleanup();
        surface.release();
        texture.release();
        activity.close();
    }

    private int submitPFrame() {
        frame++;
        byte[] picture = new byte[200];
        picture[3] = 1;
        return renderer.submitDecodeUnit(picture, picture.length, MoonBridge.BUFFER_TYPE_PICDATA, frame,
                MoonBridge.FRAME_TYPE_PFRAME, (char) 0, frame * 16_000L, frame * 16_000L, frame * 16_000_000L,
                frame * 16_000L);
    }

    private int submitIdrPicture() {
        frame++;
        byte[] picture = new byte[200];
        picture[3] = 1;
        return renderer.submitDecodeUnit(picture, picture.length, MoonBridge.BUFFER_TYPE_PICDATA, frame,
                MoonBridge.FRAME_TYPE_IDR, (char) 0, frame * 16_000L, frame * 16_000L, frame * 16_000_000L,
                frame * 16_000L);
    }

    private static void await(java.util.function.BooleanSupplier condition, String what) throws InterruptedException {
        long deadline = System.nanoTime() + 20_000_000_000L;
        while (!condition.getAsBoolean() && System.nanoTime() < deadline) Thread.sleep(1);
        assertTrue(what, condition.getAsBoolean());
    }

    @Test
    public void aDecoderWithoutAFreeBufferDoesNotHoldTheReceiveThread() throws Exception {
        long startNs = System.nanoTime();
        assertEquals(MoonBridge.DR_OK, submitPFrame());
        long tookMs = (System.nanoTime() - startNs) / 1_000_000;
        // At most the receive bound's tries, where it waited up to 5 s for a buffer before.
        assertTrue("tries " + StallingCodec.receiveThreadTries.get(),
                StallingCodec.receiveThreadTries.get() <= InputBufferWait.RECEIVE_TRIES);
        assertTrue("took " + tookMs + " ms", tookMs < 1000);
        assertTrue(StallingCodec.queuedSizes.isEmpty());

        // A second frame behind it returns at once too, and the buffer's coming frees both, in order.
        assertEquals(MoonBridge.DR_OK, submitPFrame());
        StallingCodec.free = true;
        await(() -> StallingCodec.queuedSizes.size() == 2, "both frames reach the decoder");
        assertEquals(0, drops());
    }

    @Test
    public void aDecoderWithABufferIsSubmittedInPlace() {
        StallingCodec.free = true;
        assertEquals(MoonBridge.DR_OK, submitPFrame());
        assertEquals(1, StallingCodec.queuedSizes.size());
        assertEquals(0, StallingCodec.receiveThreadTries.get());
    }

    @Test
    public void aKeyFrameNeedingTwoBuffersIsTakenByTheWorkerEvenWithAFreeDecoder() throws Exception {
        // The first key frame after a configure sends its parameter sets in a buffer of their own,
        // then the picture: a second wait that could not be handed over half way through.
        StallingCodec.free = true;
        assertEquals(MoonBridge.DR_OK, submitIdrPicture());
        await(() -> StallingCodec.queuedSizes.size() == 2, "the parameter sets and the picture reach the decoder");
        assertEquals(0, StallingCodec.receiveThreadTries.get());
        assertEquals(List.of(WORKER, WORKER), StallingCodec.queueThreads);
    }

    @Test
    public void aReallyHungDecoderStillEndsInTheHangDetectionOnTheWorker() throws Exception {
        assertEquals(MoonBridge.DR_OK, submitPFrame());
        // The worker waits out the hang limit counted from the receive thread's first try: its reset
        // is requested as the detection did on the receive thread, which is back reading packets.
        AtomicInteger recovery = ReflectionHelpers.getField(renderer, "codecRecoveryType");
        await(() -> recovery.get() != 0, "the hang detection asks for the reset");
        assertEquals(3, recovery.get());
        SubmitHandoff<?> handoff = ReflectionHelpers.getField(renderer, "submitHandoff");
        await(handoff::takeKeyGapPeek, "the failed unit makes the hand-off wait for a key frame");
        // The frame that reached the decoder's path is no drop; the next one is, with the news.
        assertEquals(0, drops());
        assertEquals(MoonBridge.DR_NEED_IDR, submitPFrame());
        assertEquals(1, drops());
        assertFalse(StallingCodec.queuedSizes.contains(200));
        assertNotNull(ReflectionHelpers.getField(renderer, "submitHandoff"));
    }

    private long drops() {
        SubmitHandoff<?> handoff = ReflectionHelpers.getField(renderer, "submitHandoff");
        return handoff.drops();
    }
}
