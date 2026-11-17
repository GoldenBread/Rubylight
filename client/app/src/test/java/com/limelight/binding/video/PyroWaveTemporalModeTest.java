package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

/**
 * The temporal upscaler on PyroWave: Java's mode and state numbers are the native renderer's
 * (presentation.h). Its choice of path and its sizes are pyrowave_presentation_test.cpp's
 * (scripts/test-native.sh).
 */
public class PyroWaveTemporalModeTest {
    private static final String PRESENTATION = "src/main/jni/pyrowave-renderer/presentation.h";

    private static String read(String path) throws Exception {
        return new String(Files.readAllBytes(Paths.get(path)), StandardCharsets.UTF_8).replace("\r\n", "\n");
    }

    @Test
    public void eachModeHasItsNativeNumber() throws Exception {
        String header = read(PRESENTATION);
        assertTrue(header.contains("constexpr int UPSCALE_OFF = 0;"));
        assertTrue(header.contains("constexpr int UPSCALE_SGSR = 1;"));
        assertTrue(header.contains("constexpr int UPSCALE_BILINEAR = 2;"));
        assertTrue(header.contains("constexpr int UPSCALE_TEMPORAL = 3;"));
        assertEquals(0, PyroWaveDecoderRenderer.upscaleMode(UpscalingPolicy.Mode.OFF));
        assertEquals(1, PyroWaveDecoderRenderer.upscaleMode(UpscalingPolicy.Mode.FSR1));
        assertEquals(1, PyroWaveDecoderRenderer.upscaleMode(UpscalingPolicy.Mode.SGSR1));
        assertEquals(2, PyroWaveDecoderRenderer.upscaleMode(UpscalingPolicy.Mode.BILINEAR));
        assertEquals(3, PyroWaveDecoderRenderer.upscaleMode(UpscalingPolicy.Mode.SGSR2));
    }

    @Test
    public void temporalStatesAreTheNativeOnes() throws Exception {
        assertTrue(read(PRESENTATION).contains(
                "enum PyroWaveTemporalState { TEMPORAL_OFF = 0, TEMPORAL_ACTIVE = 1, TEMPORAL_FALLBACK = 2 };"));
        assertEquals(0, PyroWaveDecoderRenderer.TEMPORAL_OFF);
        assertEquals(1, PyroWaveDecoderRenderer.TEMPORAL_ACTIVE);
        assertEquals(2, PyroWaveDecoderRenderer.TEMPORAL_FALLBACK);
    }
}
