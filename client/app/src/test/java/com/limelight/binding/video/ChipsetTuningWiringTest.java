package com.limelight.binding.video;

import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * Chipset tuning reaches the decoder setup again: the renderer's configure tries hand the setting to
 * setDecoderLowLatencyOptions (MediaCodecDecoderRenderer.lowLatencyOptions), on as true and off as false.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ChipsetTuningWiringTest {
    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
    }

    private static MediaCodecInfo decoder(String name) {
        // No vendor parameters listed, so no codec is opened to ask for them.
        Map<String, List<String>> parameters = ReflectionHelpers.getStaticField(MediaCodecHelper.class, "vendorParameterCache");
        parameters.put(name, new ArrayList<>());
        MediaFormat capabilities = MediaFormat.createVideoFormat("video/hevc", 3840, 2160);
        return MediaCodecInfoBuilder.newBuilder().setName(name).setIsHardwareAccelerated(true).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }

    /** The first configure try's format for this decoder, from settings with Chipset tuning on or off. */
    private static MediaFormat firstTry(String decoderName, boolean chipsetTuning) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.codecLowLatency = true;
        prefs.vendorLowLatency = true;
        prefs.codecPerformance = true;
        prefs.experimentalChipsetTuning = chipsetTuning;
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        format.setInteger(MediaFormat.KEY_FRAME_RATE, 60);
        MediaCodecDecoderRenderer.lowLatencyOptions(format, decoder(decoderName), 0, prefs, true);
        return format;
    }

    private static TreeSet<String> keys(MediaFormat format) {
        return new TreeSet<>(format.getKeys());
    }

    @Test
    public void onReachesTheDecoderOptionsAsTrueAndOffAsFalse() {
        // A Codec2 MediaTek decoder gets vdec-lowlatency only with it on.
        assertEquals(1, firstTry("c2.mtk.hevc.decoder", true).getInteger("vdec-lowlatency"));
        assertFalse(firstTry("c2.mtk.hevc.decoder", false).containsKey("vdec-lowlatency"));
        // An Exynos decoder gets the operating rate it declares only with it on; off, the priority hint alone.
        assertTrue(firstTry("c2.exynos.hevc.decoder", true).containsKey(MediaFormat.KEY_OPERATING_RATE));
        assertFalse(firstTry("c2.exynos.hevc.decoder", false).containsKey(MediaFormat.KEY_OPERATING_RATE));
        // Other chipsets get the same options either way.
        assertEquals(keys(firstTry("c2.qti.hevc.decoder", false)), keys(firstTry("c2.qti.hevc.decoder", true)));
    }

    @Test
    public void itIsOffByDefaultAndReadFromItsOriginalKey() {
        android.content.SharedPreferences prefs =
                android.preference.PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
        assertFalse(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).experimentalChipsetTuning);
        // A value users stored before keeps working and is not cleared.
        prefs.edit().putBoolean("checkbox_experimental_chipset_tuning", true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(RuntimeEnvironment.getApplication()).experimentalChipsetTuning);
        assertTrue(prefs.getBoolean("checkbox_experimental_chipset_tuning", false));
        prefs.edit().clear().commit();
    }
}
