package com.limelight.nvstream.jni;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;

/**
 * The streaming engine (nightjar-core, protocol/android in this repository), loaded from
 * libnightjar_core.so. It is the only engine: MoonBridge starts every stream here, or fails the
 * start with the reason ({@link #startRefusal}). The natives mirror MoonBridge's streaming calls
 * and call back into MoonBridge's bridge* methods, so renderers and listeners serve it.
 */
final class RustBridge {
    private RustBridge() {}

    private static volatile Boolean available;
    private static String loadFailure;

    /**
     * Loads the library once; false when it is missing or broken ({@link #loadFailure} says why).
     * MoonBridge calls it when it loads, so a failure is known before the first stream, and input
     * and queries made without the library are no-ops instead of an UnsatisfiedLinkError.
     */
    static boolean isAvailable() {
        Boolean known = available;
        return known != null ? known : load();
    }

    private static synchronized boolean load() {
        if (available == null) {
            boolean loaded;
            try {
                System.loadLibrary("nightjar_core");
                loaded = init(BuildConfig.VERSION_NAME);
                if (!loaded) loadFailure = "its initialisation failed";
            } catch (UnsatisfiedLinkError e) {
                LimeLog.warning("Streaming engine unavailable: " + e.getMessage());
                loadFailure = "the library did not load (" + e.getMessage() + ")";
                loaded = false;
            }
            available = loaded;
        }
        return available;
    }

    /** Why {@link #isAvailable} is false, or null while it is true. */
    static synchronized String loadFailure() {
        return available != null && !available ? loadFailure : null;
    }

    // GFE 3.22 (app version 7.1.431) introduced the encrypted control stream; the engine
    // does not speak the older generations.
    private static final int[] MIN_APP_VERSION = {7, 1, 431};

    /**
     * Why the engine cannot start this stream, as the sentence the connecting screen shows, or null
     * when it can. The library is checked separately ({@link #startRefusal}), so this stays
     * testable without it.
     */
    static String hostRefusal(String appVersion, int serverCodecModeSupport, boolean inputOnly) {
        if (!appVersionAtLeast(appVersion, MIN_APP_VERSION)) {
            return "This PC's streaming software is older than GFE 3.22 (app version " + appVersion +
                    "), which Nightjar does not support. Update it, or use Sunshine, Apollo or Rubylight.";
        }
        if (!inputOnly && serverCodecModeSupport == 0) {
            return "This PC reports no video codec it can encode, so the stream cannot start. " +
                    "Check the PC's GPU and streaming software.";
        }
        return null;
    }

    /** Why the next stream cannot start (the host's, or the library's), or null when it can. */
    static String startRefusal(String appVersion, int serverCodecModeSupport, boolean inputOnly) {
        String refusal = hostRefusal(appVersion, serverCodecModeSupport, inputOnly);
        if (refusal != null) return refusal;
        if (!isAvailable()) {
            return "Nightjar's streaming engine (libnightjar_core.so) cannot start: " + loadFailure() +
                    ". Reinstall Nightjar, and send a problem report if this stays.";
        }
        return null;
    }

    static boolean appVersionAtLeast(String appVersion, int[] minimum) {
        if (appVersion == null) {
            return false;
        }
        String[] parts = appVersion.split("\\.");
        for (int i = 0; i < minimum.length; i++) {
            int value;
            try {
                value = i < parts.length ? Integer.parseInt(parts[i].trim()) : 0;
            } catch (NumberFormatException e) {
                return false;
            }
            if (value != minimum[i]) {
                return value > minimum[i];
            }
        }
        return true;
    }

    /** Loads the engine; {@code appVersion} names this app in the User-Agent of requests to the host. */
    private static native boolean init(String appVersion);

    static native int startConnection(String address, String appVersion, String rtspSessionUrl,
                                      int serverCodecModeSupport, int width, int height, int fps,
                                      int bitrate, int packetSize, int streamingRemotely,
                                      int audioConfiguration, int supportedVideoFormats,
                                      int clientRefreshRateX100, byte[] riAesKey, byte[] riAesIv,
                                      int videoCapabilities, int colorSpace, int colorRange,
                                      int hostFeatures, boolean inputOnly, boolean enableMic,
                                      boolean hapticsPcm, int pyrowaveLinkMbps, int pyrowavePaceMbps,
                                      boolean clientWireless, int networkGracePeriodMs, boolean packed444,
                                      int packed444Modes, boolean remoteMonitor, boolean unbatchedInput,
                                      boolean networkPriority, boolean roaming, boolean roamingMetered,
                                      int audioPassthrough, int hdrFormat, int videoFecPercentage,
                                      boolean pyrowaveEarly, boolean codecDirect,
                                      boolean receiveDecodePriority, boolean frameWarpRequested,
                                      boolean receiveCoalescing, int decodeQueueBound);
    static native void stopConnection();
    static native void interruptConnection();
    static native int networkChanged(boolean metered);
    static native boolean hostSaidGoodbye();
    static native String getHostAddress();
    static native int hostLossRound(String host, int port, String gateway);
    static native int hostLossAfterGoodbye(boolean onLink, int round, long waitedMs);
    static native void sendMouseMove(short deltaX, short deltaY);
    static native void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight);
    static native void sendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight);
    static native void sendMouseButton(byte buttonEvent, byte mouseButton);
    static native void sendMultiControllerInput(short controllerNumber, short activeGamepadMask, int buttonFlags, byte leftTrigger, byte rightTrigger, short leftStickX, short leftStickY, short rightStickX, short rightStickY);
    static native int sendTouchEvent(byte eventType, int pointerId, float x, float y, float pressure, float contactAreaMajor, float contactAreaMinor, short rotation);
    static native int sendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y, float pressure, float contactAreaMajor, float contactAreaMinor, short rotation, byte tilt);
    static native int sendControllerIdentityEvent(byte controllerNumber, int vendorId, int productId);
    /** How the host takes a physical touchpad's fingers: {@link MoonBridge#TOUCHPAD_NONE}, _INDIVIDUAL or _FRAME. */
    static native int touchpadTransport();
    /** One motion event of a physical touchpad: the first {@code count} fingers on it, positions 0-1 on its surface. */
    static native int touchpadFrame(int device, int[] ids, float[] xs, float[] ys, float[] pressures, int count,
                                    boolean button, short widthMm, short heightMm);
    static native int touchpadCancel();
    static native int sendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type, int supportedButtonFlags, short capabilities);
    static native int sendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId, float x, float y, float pressure);
    static native int sendControllerMotionEvent(byte controllerNumber, byte motionType, float x, float y, float z);
    static native int sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage);
    static native void sendKeyboardInput(short keyMap, byte keyDirection, byte modifier, byte flags);
    static native void sendMouseHighResScroll(short scrollAmount);
    static native void sendMouseHighResHScroll(short scrollAmount);
    static native void sendUtf8Text(byte[] text);
    static native boolean sendServerCommand(byte[] payload);
    static native int getPendingAudioDuration();

    // nightjar-core's connectivity test and STUN lookup (protocol/android/src/lib.rs).
    static native int testClientConnectivity(String testServerHostName, int referencePort, int testFlags);
    static native String findExternalAddressIP4(String stunHostName, int stunPort);
    static native long audioConcealer();
    static native int getPendingVideoFrames();
    static native long getDecoderDrops();
    static native long getEstimatedRttInfo();
    static native long getWireBytesReceived();
    static native long getVideoReceiveBufferInfo();
    static native boolean reportFrameSlack(long slackNs, long periodNs);
    static native void flushPhaseLockReport();
    static native int getPhaseLockLeadUs();
    /** The session clock's microseconds now: what frame receive and enqueue times are measured on. */
    static native long sessionNowUs();
    static native boolean hostSupportsControlMessage(int messageType);
    // Frame Warp (protocol/android/src/frame_warp.rs); MoonBridge has what they mean.
    static native boolean frameWarpOffered();
    static native long frameWarpApi();
    static native void frameWarpNoteFrame(long ptsUs, int frame);
    static native int frameWarpForPts(long ptsUs, float[] out);
    static native int frameWarpPeekForPts(long ptsUs, float[] out);
    static native long frameWarpInputState();
    static native int frameWarpMask(int known, int[] size, byte[] cells);
    static native void frameWarpDecline();
    static native int frameWarpStatus(float[] out);
    static native boolean sendDisplayCaps(boolean hdr, float maxNits, float maxAverageNits, float minNits);
    static native int getMicrophoneState();
    static native int getHostPacked444Modes();
    static native int getAnnouncedHdrFormat();
    static native int sendMicrophonePcm(short[] pcm, int samples);
    static native boolean requestReconfigure(int width, int height, int fpsMillihz);
    static native boolean sendHostTimingRequest(boolean enabled);
    static native boolean sendLinkNoticeRequest(boolean enabled);
    static native boolean sendClockSyncProbe();
    static native void presentClockStart(int minIntervalUs, boolean adaptive);
    static native void presentClockStop();
    static native void presentClockReset();
    static native boolean presentClockSchedule(long hostUs, long readyNs, long[] out);
    static native boolean presentClockStats(long[] out);
    static native boolean presentClockOffset(long[] out);
    static native boolean presentClockScheduleOnGrid(long hostUs, long readyNs, long anchorNs, long intervalNs,
            long leadNs, long slackNs, long[] out);
    static native void presentClockObserve(long hostUs);
    static native int presentClockContentRate();
    static native boolean sendCursorShapeRequest(boolean enabled);
    static native boolean sendControllerAudio(int kind, byte[] payload);
    static native boolean sendTextInputRequest(boolean enabled);
    static native boolean sendRateChange(int serial, int bitrateKbps, int fpsMillihz);
    static native boolean sendKeyboardLayout(String bcp47Tag);
    // Clipboard sync (0x5508): MoonBridge.CLIPBOARD_SYNC_* and MoonBridge.CLIPBOARD_COPY_*.
    static native int clipboardSyncStart(String current);
    static native int clipboardSyncLocal(String text);
    // USB forwarding (0x553A): a request id, or one of MoonBridge.USB_*.
    static native int usbAttach(int fd, String path, String name, String address, int httpPort,
                                int httpsPort, String serverCertificatePem, String uniqueId,
                                byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8);
    static native boolean usbDetach(int requestId);
    static native String usbList();
}
