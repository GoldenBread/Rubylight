package com.limelight.binding.video;

import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.os.Build;
import android.view.Display;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Manages mid-stream resolution and frame-rate reconfiguration, adaptive
 * playback limits, covering-size calculations, and decoded-size notifications
 * for {@link MediaCodecDecoderRenderer}.
 */
final class MediaCodecReconfigure {
    private final MediaCodecDecoderRenderer r;

    // What the UI reads of the configured decoder (publishReconfigureLimits), never changed once published.
    volatile ReconfigureCaps reconfigureLimits;
    // A size change the UI asked for, applied by the input thread on its next frame (applyStreamChanges).
    private final AtomicReference<ResizeRequest> pendingResize = new AtomicReference<>();
    // A frame rate the UI set that the codec's format and the upscaler still have to take (applyStreamChanges).
    private final AtomicBoolean pendingFrameRate = new AtomicBoolean();
    // A rebuild cleared the limits and its configure has not published new ones yet.
    private volatile boolean reconfigureLimitsRebuilding;
    // Held while a rebuild starts or ends (reconfigureLimitsRebuilding flips) and while a UI action runs
    // (runUnlessRebuilding). A decoder thread takes it under codecRecoveryMonitor; the UI never takes that.
    private final Object reconfigureGate = new Object();
    // Told on the decoder thread when a rebuild's configure published the limits again.
    private volatile Runnable reconfigureLimitsListener;

    volatile int streamWidth, streamHeight;
    volatile MediaCodecDecoderRenderer.StreamSizeListener streamSizeListener;
    volatile MediaCodecDecoderRenderer.PackedFailureListener packedFailureListener;

    MediaCodecReconfigure(MediaCodecDecoderRenderer renderer) {
        this.r = renderer;
    }

    static final class ReconfigureCaps {
        final int[] limits;
        final MediaCodecInfo.VideoCapabilities caps;

        ReconfigureCaps(int[] limits, MediaCodecInfo.VideoCapabilities caps) {
            this.limits = limits;
            this.caps = caps;
        }
    }

    static final class ResizeRequest {
        final int width, height, maxWidth, maxHeight;
        // The request this one replaced before the input thread took it, which the PC already got.
        final ResizeRequest replaced;

        ResizeRequest(int width, int height, int maxWidth, int maxHeight, ResizeRequest replaced) {
            this.replaced = replaced;
            this.width = width;
            this.height = height;
            this.maxWidth = maxWidth;
            this.maxHeight = maxHeight;
        }
    }

    int[] getStreamSize() {
        return new int[] {streamWidth, streamHeight};
    }

    // A decoder that can change size mid-stream, or null: PyroWave (its Vulkan path sizes its
    // planes once), decoders without adaptive playback, and Android 7.1 and older, whose H.264
    // SPS fixups assume the first size.
    private MediaCodecInfo.VideoCapabilities reconfigureCapabilities() {
        if (r.stopping || r.videoDecoder == null || r.configuredFormat == null || !r.adaptivePlayback ||
                r.vulkanRenderer != null || (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ||
                Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return null;
        }
        try {
            return r.videoDecoder.getCodecInfo()
                    .getCapabilitiesForType(r.configuredFormat.getString(MediaFormat.KEY_MIME))
                    .getVideoCapabilities();
        } catch (RuntimeException e) {
            LimeLog.warning("Cannot read decoder sizes: " + e);
            return null;
        }
    }

    int[] getReconfigureLimits() {
        ReconfigureCaps published = publishedReconfigureCaps();
        return published == null ? null : published.limits.clone();
    }

    private ReconfigureCaps publishedReconfigureCaps() {
        if (r.decoderFailure != null || r.stopping || r.vulkanRenderer != null) return null;
        return reconfigureLimits;
    }

    boolean reconfigureLimitsPending() {
        return reconfigureLimitsRebuilding && r.decoderFailure == null && !r.stopping;
    }

    boolean runUnlessRebuilding(Runnable action) {
        synchronized (reconfigureGate) {
            if (r.reconfigureLimitsPending()) return false;
            action.run();
            return true;
        }
    }

    // A rebuild begins (it may move to another decoder): no size change is offered until that one is configured.
    void startRebuild() {
        synchronized (reconfigureGate) {
            reconfigureLimitsRebuilding = true;
            reconfigureLimits = null;
        }
    }

    // A configure completed with its limits published; true when it ended a rebuild.
    boolean endRebuild() {
        synchronized (reconfigureGate) {
            boolean rebuilt = reconfigureLimitsRebuilding;
            reconfigureLimitsRebuilding = false;
            return rebuilt;
        }
    }

    void setReconfigureLimitsListener(Runnable listener) {
        reconfigureLimitsListener = listener;
    }

    void notifyReconfigureLimits() {
        Runnable listener = reconfigureLimitsListener;
        if (listener != null) listener.run();
    }

    // After every configure (configureAndStartDecoder, which every setup, recovery and rebuild
    // ends with) and at cleanup: what getReconfigureLimits() hands out until the next one.
    void publishReconfigureLimits() {
        MediaCodecInfo.VideoCapabilities caps = reconfigureCapabilities();
        if (caps == null) {
            reconfigureLimits = null;
            return;
        }
        int maxWidth = caps.getSupportedWidths().getUpper();
        int maxHeight = caps.getSupportedHeights().getUpper();
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0) {
            // initializeDecoder() refuses H.264 above 4096 either way.
            maxWidth = Math.min(maxWidth, 4096);
            maxHeight = Math.min(maxHeight, 4096);
        }
        reconfigureLimits = new ReconfigureCaps(
                new int[] {maxWidth, maxHeight, caps.getWidthAlignment(), caps.getHeightAlignment()}, caps);
    }

    static int[] upFrontMax(int width, int height, int[] covering, MediaCodecInfo.VideoCapabilities caps, boolean h264) {
        if (caps == null) {
            return new int[] {width, height};
        }
        return upFrontMax(width, height, covering, caps.getWidthAlignment(), caps.getHeightAlignment(),
                caps::isSizeSupported, h264);
    }

    static int[] upFrontMax(int width, int height, int[] covering, int widthAlignment, int heightAlignment,
                            MediaCodecDecoderRenderer.SizeCheck decoder, boolean h264) {
        int[] stream = {width, height};
        if (covering == null || (covering[0] <= width && covering[1] <= height)) {
            return stream;
        }
        int[] max = StreamResizePolicy.coveringMax(width, height, covering[0], covering[1]);
        int maxWidth = alignUp(max[0], widthAlignment);
        int maxHeight = alignUp(max[1], heightAlignment);
        if ((h264 && (maxWidth > 4096 || maxHeight > 4096)) || !decoder.takes(maxWidth, maxHeight)) {
            LimeLog.info("Decoder takes no " + maxWidth + "x" + maxHeight + " maximum; it grows when asked");
            return stream;
        }
        return new int[] {maxWidth, maxHeight};
    }

    private static int alignUp(int value, int alignment) {
        return alignment > 1 ? (value + alignment - 1) / alignment * alignment : value;
    }

    boolean prepareForReconfigure(int width, int height) {
        if (r.decoderFailure != null || r.stopping) return false;
        // Read first: the configure that published it saw packing set up.
        ReconfigureCaps published = publishedReconfigureCaps();
        if (r.packing != null) {
            MoonBridge.bridgeClDisplayMessage("Reconnect to change the logical size of a packed full-chroma stream.");
            return false;
        }
        if (published == null) {
            return false;
        }
        // A request the input thread has not applied yet already raised the maximum.
        ResizeRequest waiting = pendingResize.get();
        int[] max = StreamResizePolicy.coveringMax(r.maxDecodeWidth, r.maxDecodeHeight, width, height);
        if (waiting != null) max = StreamResizePolicy.coveringMax(max[0], max[1], waiting.maxWidth, waiting.maxHeight);
        if (!published.caps.isSizeSupported(width, height) || !published.caps.isSizeSupported(max[0], max[1])) {
            LimeLog.warning("Decoder cannot switch to " + width + "x" + height + " within " +
                    max[0] + "x" + max[1]);
            return false;
        }
        pendingResize.set(new ResizeRequest(width, height, max[0], max[1], waiting));
        return true;
    }

    void cancelReconfigure() {
        // A request it replaced was sent: the decoder still takes that one.
        ResizeRequest cancelled = pendingResize.get();
        if (cancelled != null) pendingResize.compareAndSet(cancelled, cancelled.replaced);
    }

    // On the input thread at a frame's first unit: the size and frame rate the UI asked for, under
    // the monitor that a recovery holds while it touches the codec. True when the size asks for a
    // restart, which the caller runs before it keeps that frame's parameter sets.
    boolean applyStreamChanges() {
        if (pendingResize.get() == null && !pendingFrameRate.get()) return false;
        boolean restart = false;
        synchronized (r.codecRecoveryMonitor) {
            ResizeRequest resize = pendingResize.getAndSet(null);
            boolean frameRate = pendingFrameRate.getAndSet(false);
            if (r.stopping || r.configuredFormat == null) return false;
            if (resize != null) restart = applyResize(resize);
            if (frameRate) {
                r.configuredFormat.setInteger(MediaFormat.KEY_FRAME_RATE, r.refreshRate);
                Display display = r.outputDisplay;
                if (display != null) {
                    updateUpscalingFrameRate(display.getMode().getRefreshRate());
                }
            }
        }
        return restart;
    }

    private boolean applyResize(ResizeRequest resize) {
        int[] max = StreamResizePolicy.coveringMax(r.maxDecodeWidth, r.maxDecodeHeight, resize.maxWidth, resize.maxHeight);
        // Checked against the decoder running now: a rebuild since the UI asked may have moved to
        // one that cannot take it, whose configure would then refuse the raised maximum.
        ReconfigureCaps live = reconfigureLimits;
        if (live == null || !live.caps.isSizeSupported(resize.width, resize.height) ||
                !live.caps.isSizeSupported(max[0], max[1])) {
            LimeLog.warning("Decoder " + r.activeDecoderName + " cannot switch to " + resize.width + "x" + resize.height +
                    " within " + max[0] + "x" + max[1] + "; the size change is dropped");
            return false;
        }
        boolean restart = false;
        if (max[0] != r.maxDecodeWidth || max[1] != r.maxDecodeHeight) {
            LimeLog.info("Decoder restarts for sizes up to " + max[0] + "x" + max[1] +
                    " (was " + r.maxDecodeWidth + "x" + r.maxDecodeHeight + ")");
            r.maxDecodeWidth = max[0];
            r.maxDecodeHeight = max[1];
            r.configuredFormat.setInteger(MediaFormat.KEY_MAX_WIDTH, r.maxDecodeWidth);
            r.configuredFormat.setInteger(MediaFormat.KEY_MAX_HEIGHT, r.maxDecodeHeight);
            restart = true;
        }
        if (r.upscaler != null && (resize.width != r.initialWidth || resize.height != r.initialHeight)) {
            // The GLES upscaler is built for the first size; continue on direct output.
            r.upscalingPolicy.fail(UpscalingPolicy.Reason.SIZE_CHANGED);
            restart = true;
        }
        if (restart) {
            // An expected "recovery", like an HDR change.
            r.codecRecoveryAttempts = 0;
            r.session.requestRestart("size change");
        }
        r.expectedWidth = resize.width;
        r.expectedHeight = resize.height;
        r.reconfigureRequested = true;
        return restart;
    }

    void setStreamFrameRateMilliHz(int fpsMilliHz) {
        FramePacingPolicy.frameIntervalNs(fpsMilliHz); // Validate before publishing to the render threads.
        if (r.decoderFailure != null || r.stopping || fpsMilliHz == r.streamFrameRateMilliHz) return;
        r.streamFrameRateMilliHz = fpsMilliHz;
        r.frameLatencyStats.resetDecodeTiming();
        r.refreshRate = (fpsMilliHz + 500) / 1000;
        pendingFrameRate.set(true);
    }

    void updateUpscalingFrameRate(float panelHz) {
        if (r.decoderFailure != null) return;
        synchronized (r.codecRecoveryMonitor) {
            GlesUpscaler current = r.upscaler;
            if (current != null) current.setFrameRate(Math.max(1, Math.min(r.refreshRate, Math.round(panelHz))));
        }
    }

    // Logs the size each decoder output format carries and, after a reconfigure, reports a new
    // size once its frames come out.
    void updateShownSize() {
        if (r.packing != null) return;
        int[] visible;
        try {
            if (!r.outputFormat.containsKey(MediaFormat.KEY_WIDTH) || !r.outputFormat.containsKey(MediaFormat.KEY_HEIGHT)) {
                return;
            }
            visible = StreamResizePolicy.visibleSize(r.outputFormat.getInteger(MediaFormat.KEY_WIDTH),
                    r.outputFormat.getInteger(MediaFormat.KEY_HEIGHT),
                    formatInteger(r.outputFormat, "crop-left"), formatInteger(r.outputFormat, "crop-right"),
                    formatInteger(r.outputFormat, "crop-top"), formatInteger(r.outputFormat, "crop-bottom"));
        } catch (RuntimeException e) {
            LimeLog.warning("Cannot read decoded size: " + e);
            return;
        }
        int[] shown = StreamResizePolicy.shownSize(visible[0], visible[1], r.expectedWidth, r.expectedHeight);
        LimeLog.info("Decoded size: " + visible[0] + "x" + visible[1] + ", shown as " + shown[0] + "x" + shown[1]);
        MediaCodecDecoderRenderer.StreamSizeListener sizeListener = streamSizeListener;
        if (sizeListener != null) {
            int[] decoded = StreamResizePolicy.viewPaddedSize(visible, shown,
                    r.upscaler != null && r.upscalingPolicy.getReason() == UpscalingPolicy.Reason.NONE);
            sizeListener.onDecodedSize(decoded[0], decoded[1], shown[0], shown[1]);
        }
        if (!r.reconfigureRequested || (shown[0] == streamWidth && shown[1] == streamHeight)) {
            return;
        }
        LimeLog.info("Stream size changed from " + streamWidth + "x" + streamHeight + " to " +
                shown[0] + "x" + shown[1] + " (asked for " + r.expectedWidth + "x" + r.expectedHeight + ")");
        streamWidth = shown[0];
        streamHeight = shown[1];
        MediaCodecDecoderRenderer.StreamSizeListener listener = streamSizeListener;
        if (listener != null) {
            listener.onStreamSizeChanged(shown[0], shown[1]);
        }
    }

    private static int formatInteger(MediaFormat format, String key) {
        return format.containsKey(key) ? format.getInteger(key) : -1;
    }
}
