package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;

import static org.junit.Assert.*;

/**
 * Regression tests for {@link MediaCodecDecodeLoop#patchH264Sps(byte[], int)} (T8-10): SPS
 * parsing must strictly respect {@code decodeUnitLength} when handed a larger reused buffer
 * containing stale bytes past the NAL unit, and must not throw on truncated inputs.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class MediaCodecSpsPatchTest {
    // Standard H.264 High@L4.0 1920x1080 SPS payload (after NAL header 0x67).
    private static final byte[] SPS_PAYLOAD = new byte[] {
            0x64, 0x00, 0x28, (byte) 0xac, (byte) 0xd9, 0x40, 0x78, 0x02,
            0x27, (byte) 0xe5, (byte) 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00,
            0x04, 0x00, 0x00, 0x03, 0x00, (byte) 0xf0, 0x3c, 0x60, (byte) 0xc6, 0x58
    };

    private ActivityController<Activity> activity;
    private MediaCodecDecoderRenderer renderer;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        activity = Robolectric.buildActivity(Activity.class).create();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_H264;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
    }

    @After
    public void tearDown() {
        activity.close();
    }

    private static byte[] buildSpsNal(boolean threeByteStartCode) {
        byte[] prefix = threeByteStartCode
                ? new byte[] {0x00, 0x00, 0x01, 0x67}
                : new byte[] {0x00, 0x00, 0x00, 0x01, 0x67};
        byte[] nal = new byte[prefix.length + SPS_PAYLOAD.length];
        System.arraycopy(prefix, 0, nal, 0, prefix.length);
        System.arraycopy(SPS_PAYLOAD, 0, nal, prefix.length, SPS_PAYLOAD.length);
        return nal;
    }

    @Test
    public void patchH264SpsRespectsDecodeUnitLengthInReusedBuffer() {
        for (boolean threeByte : new boolean[] {false, true}) {
            byte[] exact = buildSpsNal(threeByte);
            byte[] expected = renderer.decodeLoop.patchH264Sps(Arrays.copyOf(exact, exact.length), exact.length);

            // Reused 64 KiB buffer with stale emulation-prevention sequences (00 00 03 01) past decodeUnitLength.
            byte[] reused = new byte[65536];
            System.arraycopy(exact, 0, reused, 0, exact.length);
            for (int i = exact.length; i + 3 < reused.length; i += 4) {
                reused[i] = 0x00;
                reused[i + 1] = 0x00;
                reused[i + 2] = 0x03;
                reused[i + 3] = 0x01;
            }
            byte[] tailSnapshot = Arrays.copyOfRange(reused, exact.length, reused.length);

            byte[] patched = renderer.decodeLoop.patchH264Sps(reused, exact.length);
            assertArrayEquals("patched SPS must match exact-length input", expected, patched);
            assertArrayEquals("bytes past decodeUnitLength must not be mutated by unescapeNAL",
                    tailSnapshot, Arrays.copyOfRange(reused, exact.length, reused.length));
        }
    }

    @Test
    public void patchH264SpsHandlesTruncatedInputsWithoutThrowing() {
        assertArrayEquals(new byte[0], renderer.decodeLoop.patchH264Sps(new byte[8], 0));
        assertArrayEquals(new byte[] {0x00, 0x00, 0x01},
                renderer.decodeLoop.patchH264Sps(new byte[] {0x00, 0x00, 0x01, 0x67}, 3));
        assertArrayEquals(new byte[] {0x00, 0x00, 0x01, 0x67},
                renderer.decodeLoop.patchH264Sps(new byte[] {0x00, 0x00, 0x01, 0x67, 0x64}, 4));
        assertArrayEquals(new byte[] {0x00, 0x00, 0x00, 0x01, 0x67},
                renderer.decodeLoop.patchH264Sps(new byte[] {0x00, 0x00, 0x00, 0x01, 0x67, 0x64}, 5));
    }
}
