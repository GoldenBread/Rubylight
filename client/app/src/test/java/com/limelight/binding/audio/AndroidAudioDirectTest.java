package com.limelight.binding.audio;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/**
 * Unit tests for Slice 7 Android audio hot-path optimizations: stereo AAudio direct-pointer
 * submission, combined late-skip + queued check, and forwarded {@code pendingAudioMs}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class AndroidAudioDirectTest {

    private static final class DirectHarness extends AndroidAudioRenderer {
        int pendingMsCalls;
        int checkCalls;
        int directWriteCalls;
        int directFramesWritten;
        int shortWriteCalls;
        int copyHeadCalls;
        int queuedFrames = 480;
        boolean skipLate;
        int directWriteResult = NATIVE_WRITE_DONE;
        long now = 1000L;
        boolean headphonesActive;

        DirectHarness() {
            super(null, false, new HeadphoneSink() {
                @Override public boolean write(short[] pcm, int length, float volume) { return true; }
                @Override public boolean active() { return false; }
            });
        }

        DirectHarness(HeadphoneSink sink) {
            super(null, false, sink);
        }

        void configureAAudioStereo() {
            ReflectionHelpers.setField(this, "nativeAudio", 1L);
            ReflectionHelpers.setField(this, "channels", 2);
            ReflectionHelpers.setField(this, "sampleRate", 48000);
            ReflectionHelpers.setField(this, "reserveFrames", 960);
            ReflectionHelpers.setField(this, "bufferPolicy", new AudioBufferPolicy(192, 768, 384, 0, 0));
            ReflectionHelpers.setField(this, "lastBufferCheckMs", Long.MAX_VALUE);
            ReflectionHelpers.setField(this, "nextDropReportAt", Long.MAX_VALUE);
        }

        @Override int pendingAudioMs() {
            pendingMsCalls++;
            return 0;
        }

        @Override int checkLateAndQueuedNativeAudio(int frames, int pendingFrames) {
            checkCalls++;
            return skipLate ? -1 : queuedFrames;
        }

        @Override int writeNativeAudioDirect(long pcmPtr, int frames) {
            directWriteCalls++;
            directFramesWritten += frames;
            return directWriteResult;
        }

        @Override int writeNativeAudio(short[] audioData, int length) {
            shortWriteCalls++;
            return NATIVE_WRITE_DONE;
        }

        @Override void copyNativeShorts(long pcmPtr, short[] dst, int count) {
            copyHeadCalls++;
        }

        @Override long uptimeMillis() {
            return now;
        }
    }

    @Test
    public void directPathWritesStereoAAudioWithoutPendingJniOrShortArrayCopy() {
        DirectHarness renderer = new DirectHarness();
        renderer.configureAAudioStereo();
        assertEquals(1, renderer.playDecodedAudioDirect(0x1234L, 480, 5));
        assertEquals(0, renderer.pendingMsCalls);
        assertEquals(1, renderer.checkCalls);
        assertEquals(1, renderer.directWriteCalls);
        assertEquals(240, renderer.directFramesWritten);
        assertEquals(0, renderer.shortWriteCalls);
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test
    public void directPathReturnsNegativeOneWhenAAudioInactiveOrSurround() {
        DirectHarness renderer = new DirectHarness();
        // nativeAudio == 0 -> -1 (caller stops trying direct).
        assertEquals(-1, renderer.playDecodedAudioDirect(0x1234L, 480, 0));
        renderer.configureAAudioStereo();
        ReflectionHelpers.setField(renderer, "channels", 6);
        assertEquals(-1, renderer.playDecodedAudioDirect(0x1234L, 1440, 0));
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test
    public void directPathReturnsZeroTransientFallbackForHeadphonesOrPendingCrossfade() {
        boolean[] active = {true};
        DirectHarness renderer = new DirectHarness(new AndroidAudioRenderer.HeadphoneSink() {
            @Override public boolean write(short[] pcm, int length, float volume) { return true; }
            @Override public boolean active() { return active[0]; }
        });
        renderer.configureAAudioStereo();
        assertEquals(0, renderer.playDecodedAudioDirect(0x1234L, 480, 0));
        active[0] = false;
        ReflectionHelpers.setField(renderer, "trimHeadSamples", 480);
        assertEquals(0, renderer.playDecodedAudioDirect(0x1234L, 480, 0));
        assertEquals(0, renderer.directWriteCalls);
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test
    public void directPathHandlesLateSkipAndLateDropWithoutWriting() {
        DirectHarness renderer = new DirectHarness();
        renderer.configureAAudioStereo();
        renderer.skipLate = true;
        assertEquals(1, renderer.playDecodedAudioDirect(0x1234L, 480, 10));
        assertEquals(0, renderer.directWriteCalls);

        renderer.skipLate = false;
        // 100 ms pending with full reserve exceeds 40 ms bound -> late drop.
        assertEquals(1, renderer.playDecodedAudioDirect(0x1234L, 480, 100));
        assertEquals(0, renderer.directWriteCalls);
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test
    public void shortArrayOverloadUsesForwardedPendingMsWithoutJniCall() {
        DirectHarness renderer = new DirectHarness();
        renderer.configureAAudioStereo();
        renderer.playDecodedAudio(new short[480], 480, 4);
        assertEquals(0, renderer.pendingMsCalls);
        assertEquals(1, renderer.checkCalls);
        assertEquals(1, renderer.shortWriteCalls);
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }
}
