package com.limelight.binding.video;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Debug;
import android.os.Process;
import android.preference.PreferenceManager;
import android.view.Surface;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

import java.io.File;
import java.nio.ByteBuffer;
import java.util.Locale;

// Adapted from joemossjr16/artemis-android-pyrowave 387d3a5c (GPL-3.0).
public final class PyroWaveDecoderRenderer {
    private static final boolean LIBRARY_LOADED = loadLibrary();
    private long handle;
    // Frame Warp for this stream (setFrameWarp), until cleanup
    private boolean frameWarp;
    private int format;
    private DecoderPerformanceHints performanceHints;
    private Thread submissionThread;
    private Context maxClocksContext;
    private Context driverContext;
    private GpuDriverStore.Driver customDriver;
    private String driverStatus = "";
    private volatile long lastSubmissionDurationNs;
    private volatile float lastRecordLossPercent;
    private final long[] eventBuf = new long[128 * FrameLatencyStats.PYROWAVE_EVENT_STRIDE];
    // Native work that changes or needs the renderer (submit, early chunks, event polling, HDR and Frame Warp
    // setup) runs under nativeLock; the object monitor guards only the cheap state the getters read (handle,
    // format, driver text) and is not held across a native submit, so the statistics and health getters never wait
    // for one. Teardown (setup and cleanup) takes nativeLock first and the monitor second, always in that order:
    // it waits for the submit in flight, and only then clears handle and destroys the renderer, so a submit that
    // went native keeps its renderer alive and a later one sees handle 0. The getters check handle and call native
    // under the monitor, which the teardown also holds while it destroys.
    private final Object nativeLock = new Object();
    private final EarlyWork earlyWork = new EarlyWork();
    // Survives a crash, so the next stream can hand GPU power management back to the kernel.
    private static final String MAX_CLOCKS_ACTIVE = "pyrowave_max_clocks_active";

    private static boolean loadLibrary() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false;
        if (!Process.is64Bit()) return false;
        try {
            System.loadLibrary("pyrowave-renderer");
            return true;
        } catch (UnsatisfiedLinkError e) {
            LimeLog.info("PyroWave unavailable: " + e.getMessage());
            return false;
        }
    }

    /** 0 when PyroWave can run on this device, else the string that says why it cannot. */
    static int unavailableReason(Context context) {
        int readiness = getReadiness(context, false);
        return readiness == 0 ? 0 : failureSummary(readiness);
    }

    /**
     * The string that says why this device can never run PyroWave, whatever its GPU driver, or 0
     * when the GPU decides (Android 10, a 64-bit app and the renderer library are there).
     */
    public static int neverAvailableReason() {
        int readiness = appReadiness();
        return readiness == 0 ? 0 : failureSummary(readiness);
    }

    private static int appReadiness() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return -1;
        if (!Process.is64Bit()) return -2;
        if (!LIBRARY_LOADED) return -3;
        return 0;
    }

    private static int getReadiness(Context context, boolean hdr) {
        int app = appReadiness();
        if (app != 0) return app;
        GpuDriverStore.Driver driver = GpuDriverStore.isSupported() ? GpuDriverStore.selected(context) : null;
        if (driver != null && customDriverReadiness(context, driver, hdr) == 0) return 0;
        return nativeGetReadiness(hdr, null, null, null);
    }

    // Native codes a custom driver fails with before any device check (device_path.h), and the
    // stream's own failure after its check passed.
    static final int DRIVER_NOT_OPENED = 10;
    static final int DRIVER_NOT_LOADED = 11;
    static final int DRIVER_NOT_STARTED = 12;
    static final int DRIVER_NO_GPU = 13;
    static final int DRIVER_NOT_VULKAN = 14;
    static final int DRIVER_STREAM_FAILED = 100;
    private static final int PROBE_FAILED = 9;
    private static final Object DRIVER_LOG_LOCK = new Object();
    private static String loggedDriverFailure;

    /**
     * The custom driver's own readiness, 0 when it can decode PyroWave, else why not. The native side
     * checks each driver once per process and format; a failure is recorded for the fallback notice
     * and logged once with the dynamic linker's message when the driver did not load.
     */
    private static int customDriverReadiness(Context context, GpuDriverStore.Driver driver, boolean hdr) {
        if (!GpuDriverStore.beginUse(context, driver)) {
            LimeLog.warning("Custom GPU driver skipped: its in-use marker could not be saved");
            return PROBE_FAILED;
        }
        String hooks = context.getApplicationInfo().nativeLibraryDir;
        String directory = driver.directory.getAbsolutePath() + File.separator;
        int result = nativeGetReadiness(hdr, hooks, directory, driver.libraryName);
        GpuDriverStore.endUse(context, driver, result);
        if (result != 0) {
            String error = nativeGetCustomDriverError(hooks, directory, driver.libraryName);
            String failure = customDriverFailureLog(result, error);
            String key = driver.id + ' ' + hdr + ' ' + failure;
            boolean log;
            synchronized (DRIVER_LOG_LOCK) {
                log = !key.equals(loggedDriverFailure);
                loggedDriverFailure = key;
            }
            if (log) LimeLog.warning("Custom GPU driver cannot decode PyroWave: " + failure + "; checking the system driver");
        }
        return result;
    }

    /** What a custom driver's failure code means, in English for the log. error is the linker's message, if any. */
    static String customDriverFailureLog(int reason, String error) {
        String text;
        switch (reason) {
            case DRIVER_NOT_OPENED: text = "Android did not open the Vulkan loader with the driver hooks"; break;
            case DRIVER_NOT_LOADED: text = "its library did not load"; break;
            case DRIVER_NOT_STARTED: text = "its library loaded but Vulkan did not start on it"; break;
            case DRIVER_NO_GPU: text = "Vulkan started on it but it listed no GPU"; break;
            case DRIVER_NOT_VULKAN: text = "its library is not an Android Vulkan driver"; break;
            case DRIVER_STREAM_FAILED: text = "the stream's renderer did not start on it"; break;
            default: text = "a GPU check failed (reason " + reason + ")"; break;
        }
        return error == null || error.isEmpty() ? text : text + " (" + error + ")";
    }

    /** The notice for a custom driver that failed with reason, after which the system driver is used. */
    static int customDriverFailureSummary(int reason) {
        switch (reason) {
            case DRIVER_NOT_OPENED: return R.string.gpu_driver_failed_open;
            case DRIVER_NOT_LOADED: return R.string.gpu_driver_failed_load;
            case DRIVER_NOT_STARTED: return R.string.gpu_driver_failed_start;
            case DRIVER_NO_GPU: return R.string.gpu_driver_failed_no_gpu;
            case DRIVER_NOT_VULKAN: return R.string.gpu_driver_failed_not_vulkan;
            case DRIVER_STREAM_FAILED: return R.string.gpu_driver_failed_stream;
            case 8: return R.string.gpu_driver_failed_hdr;
            case 1: case 2: case 3: case 4: case 5: case 6: case 7:
                return R.string.gpu_driver_failed_features;
            default: return R.string.gpu_driver_system_fallback;
        }
    }

    public static int getReadinessSummary(Context context, boolean hdr) {
        int readiness = getReadiness(context, hdr);
        if (readiness == 0) {
            return GpuDriverStore.usedFallback(context) ? GpuDriverStore.fallbackSummary(context) : R.string.pyrowave_ready;
        }
        return failureSummary(readiness);
    }

    /**
     * Why PyroWave HDR cannot run here, after its output surface failed. A device whose HDR
     * readiness check passes still failed to create the 10-bit HDR10 swapchain.
     */
    public static int hdrFailureSummary(Context context) {
        int readiness = getReadiness(context, true);
        return readiness == 0 ? surfaceFailureSummary(nativeGetHdrSurfaceFailure(), Build.VERSION.SDK_INT)
                : failureSummary(readiness);
    }

    /**
     * Android 13 and 14 list no HDR10 surface format; HDR is then sent through the window dataspace.
     * Code 1 is no usable 10-bit format at all, 2 is the display or driver refusing that dataspace,
     * 3 the same for an HLG stream, 4 an HLG window that no longer holds it after the first frame.
     */
    static int surfaceFailureSummary(int nativeFailure, int sdkInt) {
        if (nativeFailure == 1 && sdkInt < 35) return R.string.pyrowave_hdr_needs_android_15;
        if (nativeFailure == 2) return R.string.pyrowave_hdr_display_refused;
        if (nativeFailure == 3 || nativeFailure == 4) return R.string.pyrowave_hlg_display_refused;
        return R.string.pyrowave_missing_swapchain;
    }

    /** Why the running stream's HDR output failed, 0 when it did not (a refused HDR10 or HLG dataspace). */
    static int streamHdrFailure() {
        if (!LIBRARY_LOADED) return 0;
        int failure = nativeGetHdrSurfaceFailure();
        return failure >= 2 && failure <= 4 ? surfaceFailureSummary(failure, Build.VERSION.SDK_INT) : 0;
    }

    /** The running HLG stream's window refused BT2020_HLG or no longer holds it (codes 3 and 4). */
    static boolean hlgWindowLost() {
        if (!LIBRARY_LOADED) return false;
        int failure = nativeGetHdrSurfaceFailure();
        return failure == 3 || failure == 4;
    }

    private static int failureSummary(int readiness) {
        // Native reason codes are shared with the renderer's device checks.
        switch (readiness) {
            case -1: return R.string.pyrowave_needs_android;
            case -2: return R.string.pyrowave_needs_64_bit;
            case -3: return R.string.pyrowave_decoder_unavailable;
            case 1: return R.string.pyrowave_needs_vulkan;
            case 2: return R.string.pyrowave_needs_subgroups;
            case 3: return R.string.pyrowave_missing_features;
            case 4: return R.string.pyrowave_missing_formats;
            case 5: return R.string.pyrowave_device_limits;
            case 6: return R.string.pyrowave_missing_queue;
            case 7: return R.string.pyrowave_missing_swapchain;
            case 8: return R.string.pyrowave_missing_hdr;
            default: return R.string.pyrowave_check_failed;
        }
    }

    /** Whether this device has an Adreno GPU, whose clocks the app can pin while streaming. */
    public static boolean hasAdrenoGpu() {
        return new File("/dev/kgsl-3d0").exists();
    }

    // SGSR serves single-frame GPU upscaling: it is a single pass, cheaper than FSR's two.
    static int upscaleMode(UpscalingPolicy.Mode mode) {
        return mode == UpscalingPolicy.Mode.FSR1 || mode == UpscalingPolicy.Mode.SGSR1 ? 1 :
                mode == UpscalingPolicy.Mode.BILINEAR ? 2 : 0;
    }

    /**
     * pacingMode is FramePacingPolicy.pyroWaveMode's. topRefreshRate is the display's highest refresh
     * rate, read only with the PC's frame times (Real VRR): frames are then shown no closer than one
     * refresh at it. 0 leaves that open.
     */
    boolean setup(Surface surface, int format, int width, int height, int fps, float displayRefreshRate,
                  float topRefreshRate, int pacingMode, boolean fullRange, boolean stretch,
                  UpscalingPolicy.Mode upscaling, int sharpness,
                  Context context, boolean maxClocks, boolean preacquire) {
        synchronized (nativeLock) {
            synchronized (this) {
                return setupLocked(surface, format, width, height, fps, displayRefreshRate, topRefreshRate,
                        pacingMode, fullRange, stretch, upscaling, sharpness, context, maxClocks, preacquire);
            }
        }
    }

    private boolean setupLocked(Surface surface, int format, int width, int height, int fps, float displayRefreshRate,
                                float topRefreshRate, int pacingMode, boolean fullRange, boolean stretch,
                                UpscalingPolicy.Mode upscaling, int sharpness,
                                Context context, boolean maxClocks, boolean preacquire) {
        cleanup();
        if (!LIBRARY_LOADED || surface == null || !surface.isValid()) return false;
        driverContext = context.getApplicationContext();
        customDriver = GpuDriverStore.isSupported() ? GpuDriverStore.selected(context) : null;
        boolean requestedCustom = customDriver != null;
        int driverFailure = 0;
        // A driver that failed its check would fail here too: go straight to the system driver.
        if (customDriver != null) {
            driverFailure = customDriverReadiness(context, customDriver,
                    (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0);
            if (driverFailure != 0) customDriver = null;
        }
        int attempts = customDriver != null ? 2 : 1;
        for (int attempt = 0; attempt < attempts; attempt++) {
            if (customDriver != null && !GpuDriverStore.beginUse(context, customDriver)) customDriver = null;
            try {
                handle = nativeCreate(surface, width, height, fps, displayRefreshRate, topRefreshRate, pacingMode,
                        (format & MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0,
                        (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, fullRange, stretch,
                        upscaleMode(upscaling), SgsrConstants.edgeSharpness(sharpness), preacquire,
                        customDriver != null ? context.getApplicationInfo().nativeLibraryDir : null,
                        customDriver != null ? customDriver.directory.getAbsolutePath() + File.separator : null,
                        customDriver != null ? customDriver.libraryName : null);
            } catch (IllegalArgumentException e) {
                LimeLog.warning("PyroWave surface unavailable: " + e);
            }
            if (handle != 0) break;
            if (customDriver != null) {
                GpuDriverStore.endUse(context, customDriver, DRIVER_STREAM_FAILED);
                customDriver = null;
                driverFailure = DRIVER_STREAM_FAILED;
                LimeLog.warning("Custom GPU driver initialization failed; retrying the system driver");
            }
        }
        driverStatus = customDriver != null ? customDriver.name : context.getString(
                requestedCustom ? customDriverFailureSummary(driverFailure) :
                GpuDriverStore.usedFallback(context) ? GpuDriverStore.fallbackSummary(context) : R.string.gpu_driver_system);
        this.format = handle != 0 ? format : 0;
        setMaxClocks(context, handle != 0 && maxClocks && hasAdrenoGpu());
        return handle != 0;
    }

    @SuppressLint("ApplySharedPref") // Must reach disk before a driver crash could kill the process.
    private void setMaxClocks(Context context, boolean enabled) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        boolean stale = prefs.getBoolean(MAX_CLOCKS_ACTIVE, false) && maxClocksContext == null;
        if (!enabled && !stale) return;
        if (!LIBRARY_LOADED) return;
        boolean applied = nativeSetGpuMaxClocks(enabled);
        maxClocksContext = enabled && applied ? context.getApplicationContext() : null;
        prefs.edit().putBoolean(MAX_CLOCKS_ACTIVE, maxClocksContext != null).commit();
    }

    synchronized int getFormat() {
        return format;
    }

    /**
     * Draws Frame Warp (docs/frame-warp.md) with the table at {@code api} (MoonBridge.frameWarpApi()) from the
     * next picture on. The renderer is set up before the stream says whether it offers Frame Warp, so this comes
     * after setup(). False when it cannot; the stream then goes without.
     */
    boolean setFrameWarp(long api) {
        synchronized (nativeLock) {
            frameWarp = handle != 0 && api != 0 && nativeSetFrameWarp(handle, api);
            return frameWarp;
        }
    }

    static final int SUBMIT_ACCEPTED = 1;
    static final int SUBMIT_BUSY = 2;

    // Accepted and busy are distinct from invalid input (zero) and a fatal error (negative). Completion
    // timestamps arrive separately through nativePollRenderedFrames, before release/present events.
    // Exactly one of data and directData is set: directData is the Rust engine's frame in place,
    // read by native code during the call without a copy.
    long submitFrame(byte[] data, ByteBuffer directData, int length, long ptsUs, int frameNumber,
                     long hostPresentationTimeUs, PyroWaveDisplayClock.Snapshot displayClock,
                     FrameLatencyStats stats, Context context, boolean enablePerformanceHints, int frameRate) {
        return submitFrameImpl(data, directData, 0L, length, ptsUs, frameNumber, hostPresentationTimeUs,
                displayClock, stats, context, enablePerformanceHints, frameRate);
    }

    long submitRawFrame(long dataPtr, int length, long ptsUs, int frameNumber,
                        long hostPresentationTimeUs, PyroWaveDisplayClock.Snapshot displayClock,
                        FrameLatencyStats stats, Context context, boolean enablePerformanceHints, int frameRate) {
        return submitFrameImpl(null, null, dataPtr, length, ptsUs, frameNumber, hostPresentationTimeUs,
                displayClock, stats, context, enablePerformanceHints, frameRate);
    }

    private long submitFrameImpl(byte[] data, ByteBuffer directData, long rawPtr, int length, long ptsUs,
                                 int frameNumber, long hostPresentationTimeUs,
                                 PyroWaveDisplayClock.Snapshot displayClock, FrameLatencyStats stats,
                                 Context context, boolean enablePerformanceHints, int frameRate) {
        synchronized (nativeLock) {
            lastSubmissionDurationNs = 0;
            if (handle == 0) return -1;
            Thread currentSubmissionThread = Thread.currentThread();
            if (submissionThread != currentSubmissionThread) {
                if (!setSubmissionThreadPriority(Process.THREAD_PRIORITY_URGENT_DISPLAY) &&
                        !setSubmissionThreadPriority(Process.THREAD_PRIORITY_DISPLAY)) {
                    LimeLog.warning("PyroWave submission thread priority could not be raised");
                }
                if (performanceHints != null) {
                    performanceHints.close();
                    performanceHints = null;
                }
                submissionThread = currentSubmissionThread;
            }
            if (performanceHints == null) {
                performanceHints = new DecoderPerformanceHints(context, enablePerformanceHints, frameRate);
            }
            long vsyncNs = displayClock.vsyncNs;
            long intervalNs = displayClock.intervalNs;
            boolean hintsActive = performanceHints.isActive();
            long workStartNs = hintsActive ? Debug.threadCpuTimeNanos() : 0;
            long result;
            long submissionStartNs = System.nanoTime();
            try {
                long packed = rawPtr != 0 ?
                        nativeSubmitRawFrame(handle, rawPtr, length, ptsUs, frameNumber, frameWarp,
                                hostPresentationTimeUs, vsyncNs, intervalNs) :
                        directData != null ?
                        nativeSubmitDirectFrame(handle, directData, length, ptsUs, frameNumber, frameWarp,
                                hostPresentationTimeUs, vsyncNs, intervalNs) :
                        nativeSubmitFrame(handle, data, length, ptsUs, frameNumber, frameWarp,
                                hostPresentationTimeUs, vsyncNs, intervalNs);
                lastRecordLossPercent = Float.intBitsToFloat((int) (packed >>> 32));
                result = (int) packed;
            } finally {
                lastSubmissionDurationNs = System.nanoTime() - submissionStartNs;
                if (hintsActive) {
                    performanceHints.reportWorkDuration(earlyWork.take(Debug.threadCpuTimeNanos() - workStartNs), frameRate);
                }
            }
            pollEvents(handle, stats);
            return result;
        }
    }

    private void pollEvents(long rendererHandle, FrameLatencyStats stats) {
        if (stats == null) return;
        for (;;) {
            int n = nativePollRenderedFrames(rendererHandle, eventBuf);
            if (n == 0) break;
            int count = n < 0 ? -n : n;
            stats.applyPyroWaveEvents(eventBuf, count, System.nanoTime());
            if (n > 0) break;
        }
    }

    private static boolean setSubmissionThreadPriority(int priority) {
        try {
            // Keep a stronger priority if the caller already assigned one. These are ordinary
            // scheduler nice values, matching the native presentation and completion workers.
            if (Process.getThreadPriority(Process.myTid()) > priority) {
                Process.setThreadPriority(priority);
            }
            return true;
        } catch (IllegalArgumentException | SecurityException e) {
            // Some devices deny the stronger priority. Try the fallback once, then keep decoding.
            return false;
        }
    }

    long getLastSubmissionDurationNs() {
        return lastSubmissionDurationNs;
    }

    /** Frames can wait for the display while newer ones decode, so each needs its own index. */
    static int outputIndex(long ptsUs) {
        return (int) ptsUs;  // Matches the native side's release events.
    }

    void pollRenderedFrames(FrameLatencyStats stats) {
        synchronized (nativeLock) {
            if (handle != 0) pollEvents(handle, stats);
        }
    }

    /**
     * Average ms per frame the decoder waited for free planes and the present thread waited for a
     * swapchain image, the percentage of decoded frames a newer one replaced before display, and,
     * with the PC's frame times (Real VRR), the average ms a ready frame was held for its time
     * (-1 in every other mode).
     */
    synchronized float[] getWaits() {
        float[] waits = handle != 0 ? nativeGetWaits(handle) : null;
        return waits != null && waits.length >= 4 ? waits : new float[] {-1, -1, -1, -1};
    }

    /** The overlay line for {@link #getWaits()}'s hold with the PC's frame times, or null without one. */
    static String hostTimesLine(float heldMs) {
        return heldMs >= 0 && Float.isFinite(heldMs) ?
                String.format(Locale.US, "PC frame times: held %.1f ms", heldMs) : null;
    }

    synchronized int getLastGpuDecodeUs() {
        return handle != 0 ? nativeGetLastGpuDecodeUs(handle) : 0;
    }

    synchronized String getDriver() {
        return handle != 0 ? nativeGetDriver(handle) + " · " + driverStatus : "";
    }

    synchronized UpscalingPolicy.Reason getUpscalingReason() {
        if (handle == 0) return UpscalingPolicy.Reason.DISABLED;
        switch (nativeGetUpscalingReason(handle)) {
            case 0: return UpscalingPolicy.Reason.NONE;
            case 1: return UpscalingPolicy.Reason.DISABLED;
            case 2: return UpscalingPolicy.Reason.HDR;
            case 3: return UpscalingPolicy.Reason.NO_UPSCALE;
            default: return UpscalingPolicy.Reason.GPU_ERROR;
        }
    }

    synchronized boolean hasMaxClocks() {
        return maxClocksContext != null;
    }

    synchronized String getPresentMode() {
        return handle != 0 ? nativeGetPresentMode(handle) : "unavailable";
    }

    void submitChunks(byte[] data, int length, boolean first) {
        synchronized (nativeLock) {
            if (handle == 0) return;
            boolean hintsActive = performanceHints != null && performanceHints.isActive();
            long workStartNs = hintsActive ? Debug.threadCpuTimeNanos() : 0;
            try {
                nativeSubmitChunks(handle, data, length, first);
            } finally {
                if (hintsActive) earlyWork.add(Debug.threadCpuTimeNanos() - workStartNs);
            }
        }
    }

    // The Rust engine's chunk batch in place: data is a direct buffer, read only during the call.
    void submitChunksDirect(ByteBuffer data, int length, boolean first) {
        synchronized (nativeLock) {
            if (handle == 0) return;
            boolean hintsActive = performanceHints != null && performanceHints.isActive();
            long workStartNs = hintsActive ? Debug.threadCpuTimeNanos() : 0;
            try {
                nativeSubmitChunksDirect(handle, data, length, first);
            } finally {
                if (hintsActive) earlyWork.add(Debug.threadCpuTimeNanos() - workStartNs);
            }
        }
    }

    // Raw native pointer fast path for early chunks (zero NewDirectByteBuffer upcall in Rust).
    void submitChunksRaw(long dataPtr, int length, boolean first) {
        synchronized (nativeLock) {
            if (handle == 0) return;
            boolean hintsActive = performanceHints != null && performanceHints.isActive();
            long workStartNs = hintsActive ? Debug.threadCpuTimeNanos() : 0;
            try {
                nativeSubmitChunksRaw(handle, dataPtr, length, first);
            } finally {
                if (hintsActive) earlyWork.add(Debug.threadCpuTimeNanos() - workStartNs);
            }
        }
    }

    /**
     * Thread CPU time of the early record pushes since the last frame's submit. They are that
     * frame's parse work, done while it arrived, so the next performance-hint report includes
     * them; otherwise the hint session would see only the shorter submit and lower the clocks.
     */
    static final class EarlyWork {
        private long pendingNs;

        void add(long ns) {
            pendingNs += ns;
        }

        /** A frame's submit time plus the pushes before it; the pushes start over. */
        long take(long submitNs) {
            long total = submitNs + pendingNs;
            pendingNs = 0;
            return total;
        }
    }

    /**
     * Whether a stream asks for early record push: a PyroWave stream from a host with record
     * framing hands the renderer each frame's packets while they arrive, so its records are in
     * the decoder by the frame's last packet. allowed is {@link #earlyRecordsAllowed()}.
     */
    public static boolean wantsEarlyRecords(boolean pyroWave, boolean hostRecords, boolean allowed) {
        return pyroWave && hostRecords && allowed;
    }

    /** On unless debug.nightjar.pyro_early is 0 (A/B runs against the whole-frame push). */
    public static boolean earlyRecordsAllowed() {
        return LIBRARY_LOADED && nativeEarlyRecordsAllowed();
    }

    /**
     * Early record push over the last stats interval: the percentage of frames whose records were
     * in the decoder by their last packet (-1 when none came early), and the frames whose early
     * packets fell short and were pushed whole.
     */
    synchronized float[] getEarlyRecords() {
        float[] early = handle != 0 ? nativeGetEarlyRecords(handle) : null;
        return early != null && early.length >= 2 ? early : new float[] {-1, 0};
    }

    /** The overlay line for {@link #getEarlyRecords()}; a frame that fell back is counted. */
    static String earlyRecordsLine(float[] early) {
        if (early[0] < 0) return "Early record push: no frame came early";
        String line = String.format(Locale.US, "Early record push: %.0f%% of frames", early[0]);
        return early[1] > 0 ? line + String.format(Locale.US, " | %.0f pushed whole", early[1]) : line;
    }

    synchronized float getLastRecordLossPercent() {
        return handle != 0 ? lastRecordLossPercent : 0;
    }

    void setHdrMode(boolean enabled, byte[] metadata) {
        synchronized (nativeLock) {
            if (handle != 0) nativeSetHdrMode(handle, enabled, metadata);
        }
    }

    /** The stream's HDR transfer from its ANNOUNCE: HLG, else PQ. Before the first frame. */
    void setTransfer(boolean hlg) {
        synchronized (nativeLock) {
            if (handle != 0) nativeSetTransfer(handle, hlg);
        }
    }

    void cleanup() {
        cleanup(null);
    }

    void cleanup(FrameLatencyStats stats) {
        synchronized (nativeLock) {
            synchronized (this) {
                cleanupLocked(stats);
            }
        }
    }

    private void cleanupLocked(FrameLatencyStats stats) {
        submissionThread = null;
        frameWarp = false;
        earlyWork.take(0);
        if (performanceHints != null) {
            performanceHints.close();
            performanceHints = null;
        }
        // Join the completion/presentation workers and deliver their final events before the
        // native renderer is deleted. The caller then finalizes any genuinely unfinished frames.
        // Clear ownership first: a statistics callback can throw after native destruction.
        long renderer = handle;
        handle = 0;
        format = 0;
        lastRecordLossPercent = 0;
        try {
            if (renderer != 0) {
                nativeDrain(renderer);
                if (stats != null) pollEvents(renderer, stats);
                nativeDestroy(renderer);
            }
        } finally {
            if (customDriver != null) {
                GpuDriverStore.endUse(driverContext, customDriver, 0);
                customDriver = null;
            }
            driverContext = null;
            restoreClocks();
        }
    }

    /** Hands GPU clocks back to the driver if this stream forced them to maximum. */
    synchronized void restoreClocks() {
        if (maxClocksContext == null) return;
        Context context = maxClocksContext;
        nativeSetGpuMaxClocks(false);
        maxClocksContext = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(MAX_CLOCKS_ACTIVE, false).apply();
    }

    private static native int nativeGetHdrSurfaceFailure();
    private static native int nativeGetReadiness(boolean hdr, String hooks, String directory, String name);
    private static native String nativeGetCustomDriverError(String hooks, String directory, String name);
    private static native long nativeCreate(Surface surface, int width, int height, int fps, float displayRefreshRate,
                                           float topRefreshRate, int pacingMode,
                                           boolean chroma444, boolean tenBit, boolean fullRange, boolean stretch,
                                           int upscale, float edgeSharpness, boolean preacquire,
                                           String hooks, String directory, String name);
    private static native String nativeGetDriver(long handle);
    private static native int nativeGetUpscalingReason(long handle);
    private static native boolean nativeSetGpuMaxClocks(boolean enabled);
    private static native long nativeSubmitFrame(long handle, byte[] data, int length, long ptsUs,
                                                int frameNumber, boolean frameWarp,
                                                long hostPresentationTimeUs, long vsyncNs, long intervalNs);
    private static native long nativeSubmitDirectFrame(long handle, ByteBuffer data, int length, long ptsUs,
                                                      int frameNumber, boolean frameWarp,
                                                      long hostPresentationTimeUs, long vsyncNs, long intervalNs);
    private static native long nativeSubmitRawFrame(long handle, long dataPtr, int length, long ptsUs,
                                                   int frameNumber, boolean frameWarp,
                                                   long hostPresentationTimeUs, long vsyncNs, long intervalNs);
    private static native int nativePollRenderedFrames(long handle, long[] out);
    private static native int nativeGetLastGpuDecodeUs(long handle);
    private static native float[] nativeGetWaits(long handle);
    private static native String nativeGetPresentMode(long handle);
    private static native void nativeSubmitChunks(long handle, byte[] data, int length, boolean first);
    private static native void nativeSubmitChunksDirect(long handle, ByteBuffer data, int length, boolean first);
    private static native void nativeSubmitChunksRaw(long handle, long dataPtr, int length, boolean first);
    private static native boolean nativeEarlyRecordsAllowed();
    private static native float[] nativeGetEarlyRecords(long handle);
    private static native void nativeSetHdrMode(long handle, boolean enabled, byte[] metadata);
    private static native void nativeSetTransfer(long handle, boolean hlg);
    private static native boolean nativeSetFrameWarp(long handle, long api);
    private static native void nativeDrain(long handle);
    private static native void nativeDestroy(long handle);
}
