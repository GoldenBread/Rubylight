package com.limelight.binding.video;

import android.content.Context;
import android.opengl.GLES11Ext;
import android.opengl.GLES30;

import java.io.IOException;

/**
 * Upscaling's Temporal mode on the GLES presenter (GlesUpscaler): six fragment passes per frame that reuse earlier
 * frames to rebuild detail. Ports the canonical shaders in assets/shaders/temporal_*.frag (whose headers credit the
 * SGSR 1 and SGSR 2 code they adapt) and client/tools/temporal_upscaler/upscalers.py; rust/PERFORMANCE.md "November
 * 16: a temporal upscaler for Nightjar" has the measurements. No frame of delay: each output uses the current decoded
 * frame and state from earlier frames only.
 *
 * <p>Compute shaders are not used: these fragment passes run on every GLES 3.0 device that can render to half-float
 * textures, which {@link #probe} checks.
 *
 * <p>Sizes are fixed for the object's life: GlesUpscaler ends this path with SIZE_CHANGED when the window changes,
 * and the stream size is the decoder's. So the state is reset only on the first frame.
 *
 * <p>All methods run on the GL thread with the presenter's context current.
 */
final class GlesTemporalUpscaler {
    // Texture units of the 2D inputs; unit 0 keeps the decoder's external texture.
    private static final int UNIT_A = 1;

    private final int w, h, outW, outH;
    private final int w1, h1, w2, h2, motionW, motionH;
    private int prep, down, motion, cut, accumulate, detail;
    private int prepTransform, accumulateReset;
    private int frame, cutTexture;
    private final int[] luma0 = new int[2], luma1 = new int[2], luma2 = new int[2];
    private final int[] motionTex = new int[2], history = new int[2];
    private final int[] prepFbo = new int[2], down1Fbo = new int[2], down2Fbo = new int[2];
    private final int[] motionFbo = new int[2], historyFbo = new int[2];
    private int cutFbo;
    private int cur;
    private boolean reset = true;
    private final int[] names = new int[1];

    /**
     * Why the temporal passes cannot run, from GL_VERSION and GL_EXTENSIONS of a GLES 3 context; null when they can.
     * The passes render to R16F and RGBA16F targets, which GLES 3.0 allows only with GL_EXT_color_buffer_half_float
     * or GL_EXT_color_buffer_float and GLES 3.2 always allows. Framebuffer completeness and the shaders are checked
     * as the passes are built.
     */
    static String probe(String glVersion, String extensions) {
        int[] version = glesVersion(glVersion);
        if (version == null) return "unknown GLES version \"" + glVersion + "\"";
        if (version[0] < 3) return "GLES " + version[0] + "." + version[1] + " is below 3.0";
        if (version[0] > 3 || version[1] >= 2) return null;
        if (hasExtension(extensions, "GL_EXT_color_buffer_half_float") ||
                hasExtension(extensions, "GL_EXT_color_buffer_float")) return null;
        return "no half-float render targets (GLES " + version[0] + "." + version[1] +
                " without GL_EXT_color_buffer_half_float or GL_EXT_color_buffer_float)";
    }

    /** Major and minor of a GLES GL_VERSION string ("OpenGL ES 3.2 V@..."), null when it has none. */
    static int[] glesVersion(String glVersion) {
        if (glVersion == null) return null;
        java.util.regex.Matcher m = java.util.regex.Pattern.compile("OpenGL ES (\\d+)\\.(\\d+)").matcher(glVersion);
        if (!m.find()) return null;
        try {
            return new int[] {Integer.parseInt(m.group(1)), Integer.parseInt(m.group(2))};
        } catch (NumberFormatException e) {
            return null;
        }
    }

    static boolean hasExtension(String extensions, String name) {
        if (extensions == null) return false;
        for (String token : extensions.trim().split("\\s+")) {
            if (token.equals(name)) return true;
        }
        return false;
    }

    /** The accumulate pass's boxStatic: SGSR 2's scaleRatio.y, min(20, (output area / stream area)^3). */
    static float boxStatic(int width, int height, int outputWidth, int outputHeight) {
        double ratio = (double) outputWidth * outputHeight / ((double) width * height);
        return (float) Math.min(20.0, ratio * ratio * ratio);
    }

    GlesTemporalUpscaler(int width, int height, int outputWidth, int outputHeight) {
        w = width;
        h = height;
        outW = outputWidth;
        outH = outputHeight;
        w1 = Math.max(1, w / 2);
        h1 = Math.max(1, h / 2);
        w2 = Math.max(1, w1 / 2);
        h2 = Math.max(1, h1 / 2);
        motionW = (w + 7) / 8;
        motionH = (h + 7) / 8;
    }

    /**
     * Probes the context and builds the programs and targets; throws IllegalStateException with the reason when the
     * device or a shader refuses, after which the caller calls {@link #release()}.
     */
    void build(Context context, String vertex, float[] sgsrViewport, float edgeSharpness) throws IOException {
        String refusal = probe(GLES30.glGetString(GLES30.GL_VERSION), GLES30.glGetString(GLES30.GL_EXTENSIONS));
        if (refusal != null) throw new IllegalStateException(refusal);
        prep = GlesUpscaler.program(vertex, GlesUpscaler.shaderSource(context, "temporal_prep.frag"));
        down = GlesUpscaler.program(vertex, GlesUpscaler.shaderSource(context, "temporal_down.frag"));
        motion = GlesUpscaler.program(vertex, GlesUpscaler.shaderSource(context, "temporal_motion.frag"));
        cut = GlesUpscaler.program(vertex, GlesUpscaler.shaderSource(context, "temporal_cut.frag"));
        accumulate = GlesUpscaler.program(vertex, GlesUpscaler.shaderSource(context, "temporal_accumulate.frag"));
        detail = GlesUpscaler.program(vertex, GlesUpscaler.shaderSource(context, "temporal_detail.frag"));

        GLES30.glUseProgram(prep);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(prep, "source"), 0);
        GLES30.glUniform2f(GLES30.glGetUniformLocation(prep, "renderSize"), w, h);
        prepTransform = GLES30.glGetUniformLocation(prep, "textureTransform");
        GLES30.glUseProgram(down);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(down, "level"), UNIT_A);
        GLES30.glUseProgram(motion);
        String[] motionInputs = {"cur0", "cur1", "cur2", "prev0", "prev1", "prev2", "prevMotion"};
        for (int i = 0; i < motionInputs.length; i++) {
            GLES30.glUniform1i(GLES30.glGetUniformLocation(motion, motionInputs[i]), UNIT_A + i);
        }
        GLES30.glUseProgram(cut);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(cut, "motion"), UNIT_A);
        GLES30.glUseProgram(accumulate);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(accumulate, "frame"), UNIT_A);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(accumulate, "motion"), UNIT_A + 1);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(accumulate, "cut"), UNIT_A + 2);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(accumulate, "history"), UNIT_A + 3);
        GLES30.glUniform2f(GLES30.glGetUniformLocation(accumulate, "renderSize"), w, h);
        GLES30.glUniform2f(GLES30.glGetUniformLocation(accumulate, "outputSize"), outW, outH);
        GLES30.glUniform1f(GLES30.glGetUniformLocation(accumulate, "boxStatic"), boxStatic(w, h, outW, outH));
        accumulateReset = GLES30.glGetUniformLocation(accumulate, "reset");
        GLES30.glUseProgram(detail);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(detail, "frame"), UNIT_A);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(detail, "history"), UNIT_A + 1);
        GLES30.glUniform4fv(GLES30.glGetUniformLocation(detail, "viewportInfo"), 1, sgsrViewport, 0);
        GLES30.glUniform2f(GLES30.glGetUniformLocation(detail, "outputSize"), outW, outH);
        GLES30.glUniform1f(GLES30.glGetUniformLocation(detail, "edgeSharpness"), edgeSharpness);

        GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + UNIT_A);
        frame = texture(GLES30.GL_RGBA8, w, h, GLES30.GL_LINEAR);
        cutTexture = texture(GLES30.GL_R16F, 1, 1, GLES30.GL_NEAREST);
        for (int i = 0; i < 2; i++) {
            luma0[i] = texture(GLES30.GL_R16F, w, h, GLES30.GL_NEAREST);
            luma1[i] = texture(GLES30.GL_R16F, w1, h1, GLES30.GL_NEAREST);
            luma2[i] = texture(GLES30.GL_R16F, w2, h2, GLES30.GL_NEAREST);
            motionTex[i] = texture(GLES30.GL_RGBA16F, motionW, motionH, GLES30.GL_LINEAR);
            history[i] = texture(GLES30.GL_RGBA16F, outW, outH, GLES30.GL_LINEAR);
        }
        GLES30.glActiveTexture(GLES30.GL_TEXTURE0);
        // Storage starts undefined: every target is cleared to 0, the motion predictor above all.
        GLES30.glClearColor(0f, 0f, 0f, 0f);
        for (int i = 0; i < 2; i++) {
            prepFbo[i] = framebuffer("temporal prep", frame, luma0[i]);
            down1Fbo[i] = framebuffer("temporal luma 1/2", luma1[i], 0);
            down2Fbo[i] = framebuffer("temporal luma 1/4", luma2[i], 0);
            motionFbo[i] = framebuffer("temporal motion", motionTex[i], 0);
            historyFbo[i] = framebuffer("temporal history", history[i], 0);
        }
        cutFbo = framebuffer("temporal cut", cutTexture, 0);
        GLES30.glClearColor(0f, 0f, 0f, 1f);
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0);
        int error = GLES30.glGetError();
        if (error != GLES30.GL_NO_ERROR) throw new IllegalStateException("GL error 0x" + Integer.toHexString(error) +
                " setting up the temporal passes");
    }

    private int texture(int format, int width, int height, int filter) {
        GLES30.glGenTextures(1, names, 0);
        GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, names[0]);
        GlesUpscaler.textureParameters(GLES30.GL_TEXTURE_2D, filter);
        GLES30.glTexStorage2D(GLES30.GL_TEXTURE_2D, 1, format, width, height);
        return names[0];
    }

    /** A framebuffer on one or two colour textures, checked complete and cleared; it stays bound. */
    private int framebuffer(String name, int first, int second) {
        GLES30.glGenFramebuffers(1, names, 0);
        int fbo = names[0];
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, fbo);
        GLES30.glFramebufferTexture2D(GLES30.GL_FRAMEBUFFER, GLES30.GL_COLOR_ATTACHMENT0, GLES30.GL_TEXTURE_2D, first, 0);
        if (second != 0) {
            GLES30.glFramebufferTexture2D(GLES30.GL_FRAMEBUFFER, GLES30.GL_COLOR_ATTACHMENT1, GLES30.GL_TEXTURE_2D,
                    second, 0);
            GLES30.glDrawBuffers(2, new int[] {GLES30.GL_COLOR_ATTACHMENT0, GLES30.GL_COLOR_ATTACHMENT1}, 0);
        }
        int status = GLES30.glCheckFramebufferStatus(GLES30.GL_FRAMEBUFFER);
        if (status != GLES30.GL_FRAMEBUFFER_COMPLETE) {
            throw new IllegalStateException(name + " framebuffer incomplete (0x" + Integer.toHexString(status) + ")");
        }
        GLES30.glClear(GLES30.GL_COLOR_BUFFER_BIT);
        return fbo;
    }

    private static void bind(int unit, int texture) {
        GLES30.glActiveTexture(GLES30.GL_TEXTURE0 + unit);
        GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, texture);
    }

    private static void pass(int fbo, int x, int y, int width, int height) {
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, fbo);
        GLES30.glViewport(x, y, width, height);
        GLES30.glDrawArrays(GLES30.GL_TRIANGLES, 0, 3);
    }

    /**
     * Runs the six passes for the decoded frame on unit 0's external texture {@code source}, sampled through
     * {@code transform} (crop included), and draws the result into the window's {@code viewport}.
     */
    void render(int source, float[] transform, int[] viewport) {
        int prev = 1 - cur;
        // 1. The frame at stream size, and its luma
        GLES30.glActiveTexture(GLES30.GL_TEXTURE0);
        GLES30.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, source);
        GLES30.glUseProgram(prep);
        GLES30.glUniformMatrix4fv(prepTransform, 1, false, transform, 0);
        pass(prepFbo[cur], 0, 0, w, h);
        // 2. The luma pyramid
        GLES30.glUseProgram(down);
        bind(UNIT_A, luma0[cur]);
        pass(down1Fbo[cur], 0, 0, w1, h1);
        bind(UNIT_A, luma1[cur]);
        pass(down2Fbo[cur], 0, 0, w2, h2);
        // 3-4. Motion and the scene-cut mean, except on a reset, where (as upscalers.py) nothing is estimated and the
        // motion predictor stays zero.
        if (!reset) {
            GLES30.glUseProgram(motion);
            bind(UNIT_A, luma0[cur]);
            bind(UNIT_A + 1, luma1[cur]);
            bind(UNIT_A + 2, luma2[cur]);
            bind(UNIT_A + 3, luma0[prev]);
            bind(UNIT_A + 4, luma1[prev]);
            bind(UNIT_A + 5, luma2[prev]);
            bind(UNIT_A + 6, motionTex[prev]);
            pass(motionFbo[cur], 0, 0, motionW, motionH);
            GLES30.glUseProgram(cut);
            bind(UNIT_A, motionTex[cur]);
            pass(cutFbo, 0, 0, 1, 1);
        }
        // 5. History at output size
        GLES30.glUseProgram(accumulate);
        GLES30.glUniform1f(accumulateReset, reset ? 1f : 0f);
        bind(UNIT_A, frame);
        bind(UNIT_A + 1, motionTex[cur]);
        bind(UNIT_A + 2, cutTexture);
        bind(UNIT_A + 3, history[prev]);
        pass(historyFbo[cur], 0, 0, outW, outH);
        // 6. History plus the current frame's edge detail, to the window
        GLES30.glUseProgram(detail);
        bind(UNIT_A, frame);
        bind(UNIT_A + 1, history[cur]);
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0);
        GLES30.glClear(GLES30.GL_COLOR_BUFFER_BIT);
        pass(0, viewport[0], viewport[1], viewport[2], viewport[3]);
        GLES30.glActiveTexture(GLES30.GL_TEXTURE0);
        reset = false;
        cur = prev;
    }

    /** Starts over on the next frame, as after a picture change: history and the motion predictor are dropped. */
    void reset() {
        if (reset) return;
        reset = true;
        // The predictor the next estimate reads is the one written last, motionTex[1 - cur]
        GLES30.glClearColor(0f, 0f, 0f, 0f);
        for (int fbo : motionFbo) {
            GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, fbo);
            GLES30.glClear(GLES30.GL_COLOR_BUFFER_BIT);
        }
        GLES30.glClearColor(0f, 0f, 0f, 1f);
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0);
    }

    /** Deletes what was made; safe after a build() that threw, as names never made are 0. */
    void release() {
        for (int program : new int[] {prep, down, motion, cut, accumulate, detail}) {
            if (program != 0) GLES30.glDeleteProgram(program);
        }
        prep = down = motion = cut = accumulate = detail = 0;
        int[] fbos = {prepFbo[0], prepFbo[1], down1Fbo[0], down1Fbo[1], down2Fbo[0], down2Fbo[1], motionFbo[0],
                motionFbo[1], historyFbo[0], historyFbo[1], cutFbo};
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0);
        GLES30.glDeleteFramebuffers(fbos.length, fbos, 0);
        int[] textures = {frame, cutTexture, luma0[0], luma0[1], luma1[0], luma1[1], luma2[0], luma2[1],
                motionTex[0], motionTex[1], history[0], history[1]};
        GLES30.glDeleteTextures(textures.length, textures, 0);
    }
}
