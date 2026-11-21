package com.limelight.binding.video;

import org.junit.Test;

import static org.junit.Assert.*;

public class GlesUpscalerCapabilityTest {
    @Test
    public void tenBitOutputNeedsTheBt2020PqExtensionAndATenBitColourBuffer() {
        String extensions = "EGL_KHR_image EGL_EXT_gl_colorspace_bt2020_pq EGL_ANDROID_presentation_time";
        assertTrue(GlesUpscaler.tenBitOutputSupported(extensions, true));
        assertFalse(GlesUpscaler.tenBitOutputSupported(extensions, false));
        assertFalse(GlesUpscaler.tenBitOutputSupported("EGL_KHR_image", true));
        assertFalse(GlesUpscaler.tenBitOutputSupported(null, true));
    }

    @Test
    public void temporalUpscalingNeedsHalfFloatRenderTargets() {
        String adreno30 = "OpenGL ES 3.0 V@415.0 (GIT@663be55, I724753c5e3)";
        assertNull(GlesTemporalUpscaler.probe(adreno30, "GL_OES_EGL_image_external_essl3 GL_EXT_color_buffer_half_float"));
        assertNull(GlesTemporalUpscaler.probe(adreno30, "GL_EXT_color_buffer_float GL_OES_texture_float"));
        assertNotNull(GlesTemporalUpscaler.probe(adreno30, "GL_OES_EGL_image_external_essl3"));
        assertNotNull("a longer name is not the extension",
                GlesTemporalUpscaler.probe(adreno30, "GL_EXT_color_buffer_half_float_rgb"));
        assertNotNull(GlesTemporalUpscaler.probe(adreno30, null));
        assertNotNull(GlesTemporalUpscaler.probe("OpenGL ES 3.1 Mali-G78", ""));
        assertNull(GlesTemporalUpscaler.probe("OpenGL ES 3.1 Mali-G78", "GL_EXT_color_buffer_float"));
        assertNull("GLES 3.2 renders to half float in core", GlesTemporalUpscaler.probe("OpenGL ES 3.2 V@0502.0", ""));
        assertNull(GlesTemporalUpscaler.probe("OpenGL ES 3.2 V@0502.0", null));
        assertNotNull(GlesTemporalUpscaler.probe("OpenGL ES 2.0", "GL_EXT_color_buffer_half_float"));
        assertNotNull(GlesTemporalUpscaler.probe(null, "GL_EXT_color_buffer_half_float"));
        assertNotNull(GlesTemporalUpscaler.probe("garbage", "GL_EXT_color_buffer_half_float"));
        assertArrayEquals(new int[] {3, 2}, GlesTemporalUpscaler.glesVersion("OpenGL ES 3.2 V@0502.0"));
        assertNull(GlesTemporalUpscaler.glesVersion("OpenGL 4.6"));
    }

    @Test
    public void temporalBoxGrowsWithTheCubeOfTheAreaRatioUpToTwenty() {
        assertEquals(1f, GlesTemporalUpscaler.boxStatic(1920, 1080, 1920, 1080), 1e-6f);
        // 720p to 1080p: (2.25)^3
        assertEquals(11.390625f, GlesTemporalUpscaler.boxStatic(1280, 720, 1920, 1080), 1e-4f);
        assertEquals(20f, GlesTemporalUpscaler.boxStatic(1280, 720, 2560, 1440), 0f);
    }

    @Test
    public void frameWarpDrawsThePictureAgainOnceItsShiftMovedAQuarterPixel() {
        assertFalse(GlesUpscaler.needsRedraw(10f, -4f, 10f, -4f));
        assertFalse(GlesUpscaler.needsRedraw(10f, -4f, 10.1f, -4.1f));
        assertTrue(GlesUpscaler.needsRedraw(10f, -4f, 10.25f, -4f));
        assertTrue(GlesUpscaler.needsRedraw(10f, -4f, 10.2f, -4.2f));
        assertTrue("a shift that ended is drawn away", GlesUpscaler.needsRedraw(3f, 0f, 0f, 0f));
    }

    @Test
    public void frameWarpLooksAgainOnlyWhileTheModelIsLockedAndSomethingMoves() {
        long deflected = com.limelight.nvstream.jni.MoonBridge.FRAME_WARP_STICK_DEFLECTED;
        assertFalse("unlocked: nothing to redraw, whatever is sent", GlesUpscaler.mayMove(false, 0f, 0f, 4, 9 | deflected));
        assertFalse("locked and still", GlesUpscaler.mayMove(true, 0f, 0f, 4, 4));
        assertTrue("input sent since the draw", GlesUpscaler.mayMove(true, 0f, 0f, 4, 5));
        assertTrue("a deflected stick turns without new input", GlesUpscaler.mayMove(true, 0f, 0f, 4, 4 | deflected));
        assertTrue("a shift on screen can still move", GlesUpscaler.mayMove(true, 0f, -2.5f, 4, 4));
        // The count is the low 32 bits, and wraps
        assertFalse(GlesUpscaler.mayMove(true, 0f, 0f, -1, 0xFFFF_FFFFL));
        assertTrue(GlesUpscaler.mayMove(true, 0f, 0f, -1, 0L));
    }

    @Test
    public void aFenceWaitThatEndedWithoutTheSignalGoesOnAtOnceAfterAFullSliceAndPausesAfterAnEarlyReturn() {
        assertEquals(0, GlesUpscaler.repostDelayMs(4_000_000L));
        assertEquals(0, GlesUpscaler.repostDelayMs(2_000_000L));
        assertEquals("a driver that ignores the timeout must not spin the thread", 1, GlesUpscaler.repostDelayMs(0));
        assertEquals(1, GlesUpscaler.repostDelayMs(100_000L));
    }
}
