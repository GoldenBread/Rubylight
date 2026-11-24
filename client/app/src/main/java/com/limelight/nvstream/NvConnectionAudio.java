/**
 * Android microphone capture, AppOps permission checks, audio passthrough codecs, and
 * controller-vs-phone microphone route delegation onto {@link NightjarSessionJni} for
 * {@link NvConnection}.
 */
package com.limelight.nvstream;

import android.Manifest;
import android.app.AppOpsManager;
import android.content.Context;
import android.content.pm.PackageManager;
import android.media.AudioManager;
import android.os.Build;
import android.os.Process;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.binding.audio.AudioPassthrough;
import com.limelight.binding.audio.MicProcessing;
import com.limelight.binding.audio.MicRoute;
import com.limelight.binding.audio.MicrophoneCapture;
import com.limelight.binding.audio.MicrophoneStatus;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarSessionJni;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.Collection;

final class NvConnectionAudio {
    private static final int RECORD_AUDIO_CHECK = 0;
    private static final int RECORD_AUDIO_START = 1;
    private static final int RECORD_AUDIO_FINISH = 2;

    private NvConnectionAudio() {
    }

    static boolean microphoneWanted(PreferenceConfiguration prefs, NvApp app) {
        return NightjarSessionJni.microphoneWanted(
                prefs.enableMic, app.isMicOnly(), app.getRole() == NvApp.Role.REMOTE_MONITOR);
    }

    static MicProcessing microphoneProcessing(MicProcessing chosen, NvApp app) {
        if (!NightjarSessionJni.micDisablesEchoCancellation(app.isMicOnly())
                || chosen.echo == MicProcessing.Echo.OFF) {
            return chosen;
        }
        return new MicProcessing(
                MicProcessing.Echo.OFF,
                chosen.noiseSuppression,
                chosen.automaticGain,
                chosen.gainPercent,
                chosen.engine);
    }

    static boolean hasMicrophonePermission(Context appContext) {
        return appContext.checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                == PackageManager.PERMISSION_GRANTED;
    }

    static int recordAudioOp(Context appContext, int action) {
        AppOpsManager ops =
                (AppOpsManager) appContext.getSystemService(Context.APP_OPS_SERVICE);
        if (ops == null) return AppOpsManager.MODE_ERRORED;
        int uid = Process.myUid();
        String op = AppOpsManager.OPSTR_RECORD_AUDIO;
        String packageName = appContext.getPackageName();
        try {
            if (action == RECORD_AUDIO_CHECK) return ops.checkOpNoThrow(op, uid, packageName);
            if (action == RECORD_AUDIO_START) return ops.startOpNoThrow(op, uid, packageName);
            ops.finishOp(op, uid, packageName);
            return AppOpsManager.MODE_ALLOWED;
        } catch (IllegalArgumentException | SecurityException e) {
            LimeLog.warning("Microphone app-op unavailable: " + e);
            return AppOpsManager.MODE_ERRORED;
        }
    }

    static boolean microphoneCapturePermitted(NvConnection conn) {
        if (!conn.micPermitted
                || conn.stopRequested
                || (!conn.context.streamConfig.getApp().isMicOnly() && !conn.canSendInput())) {
            return false;
        }
        long now = SystemClock.uptimeMillis();
        if (conn.microphoneAccessAt != 0 && now - conn.microphoneAccessAt < 100) {
            return conn.microphoneAccess;
        }
        AudioManager audio =
                (AudioManager) conn.appContext.getSystemService(Context.AUDIO_SERVICE);
        int mode = recordAudioOp(conn.appContext, RECORD_AUDIO_CHECK);
        conn.microphoneAccess =
                hasMicrophonePermission(conn.appContext)
                        && audio != null
                        && !audio.isMicrophoneMute()
                        && (mode == AppOpsManager.MODE_ALLOWED
                                || mode == AppOpsManager.MODE_DEFAULT);
        conn.microphoneAccessAt = now;
        return conn.microphoneAccess;
    }

    static boolean beginControllerMicrophone(NvConnection conn) {
        if (!conn.controllerMicrophoneEnabled()) return false;
        return recordAudioOp(conn.appContext, RECORD_AUDIO_START) == AppOpsManager.MODE_ALLOWED;
    }

    static void endControllerMicrophone(NvConnection conn) {
        recordAudioOp(conn.appContext, RECORD_AUDIO_FINISH);
    }

    static int snapshot(NvConnection conn) {
        Collection<NvConnection.ControllerMicrophoneState> vals = conn.controllerMicrophones.values();
        int n = vals.size();
        boolean[] allowed = new boolean[n];
        boolean[] ready = new boolean[n];
        boolean[] hostMuted = new boolean[n];
        int i = 0;
        for (NvConnection.ControllerMicrophoneState s : vals) {
            allowed[i] = s.allowed;
            ready[i] = s.ready;
            hostMuted[i] = s.hostMuted;
            i++;
        }
        return NightjarSessionJni.micRouteSnapshot(conn.micMuted, allowed, ready, hostMuted);
    }

    static boolean controllerMicrophoneOwnsRoute(NvConnection conn) {
        return (snapshot(conn) & 1) != 0;
    }

    static boolean isMicrophoneMutedForReconnect(NvConnection conn) {
        return (snapshot(conn) & 2) != 0;
    }

    static MicrophoneStatus getMicrophoneStatus(NvConnection conn) {
        int snap = snapshot(conn);
        int view = (snap >> 2) & 3;
        if (view == 2 || view == 3) {
            return MicrophoneStatus.of(
                    conn.micEnabled, conn.micPermitted, MoonBridge.MIC_STATE_READY, true, conn.micMuted);
        }
        if (view == 1) {
            return MicrophoneStatus.of(
                    conn.micEnabled, conn.micPermitted, MoonBridge.MIC_STATE_READY, true, true);
        }
        return MicrophoneStatus.of(
                conn.micEnabled,
                conn.micPermitted,
                conn.micPermitted ? MoonBridge.getMicrophoneState() : MoonBridge.MIC_STATE_OFF,
                conn.micMuted || (conn.microphone != null && conn.microphone.isCapturing()),
                conn.micMuted);
    }

    static void updateControllerMicrophone(
            NvConnection conn, Object owner, NvConnection.ControllerMicrophoneState state) {
        if (owner == null) return;
        MicrophoneCapture previousCapture = null;
        boolean fallback;
        synchronized (conn) {
            if (!conn.canSendControllerInput()) state = null;
            NvConnection.ControllerMicrophoneState previous = conn.controllerMicrophones.get(owner);
            if (state == null && previous != null && previous.hostMuted) {
                conn.micMuted = true;
            }
            if (state == null) conn.controllerMicrophones.remove(owner);
            else conn.controllerMicrophones.put(owner, state);
            if (controllerMicrophoneOwnsRoute(conn)) {
                previousCapture = conn.microphone;
                conn.microphone = null;
            }
            fallback =
                    !controllerMicrophoneOwnsRoute(conn)
                            && conn.microphone == null
                            && conn.nativeConnected
                            && !conn.stopRequested;
        }
        if (previousCapture != null) previousCapture.stop();
        if (fallback) startMicrophone(conn, true);
    }

    static void startMicrophone(NvConnection conn, boolean fallback) {
        synchronized (conn) {
            if (!conn.micPermitted
                    || conn.stopRequested
                    || controllerMicrophoneOwnsRoute(conn)
                    || (fallback && (conn.micMuted || !conn.microphoneCapturePermitted()))) {
                return;
            }
        }
        int state = MoonBridge.getMicrophoneState();
        if (state != MoonBridge.MIC_STATE_READY) {
            if (!fallback) {
                LimeLog.info(
                        "Microphone: not sending ("
                                + (state == MoonBridge.MIC_STATE_NOT_OFFERED
                                        ? "not offered by host"
                                        : "host did not set it up")
                                + ")");
            }
            return;
        }
        synchronized (conn) {
            if (conn.stopRequested
                    || controllerMicrophoneOwnsRoute(conn)
                    || conn.microphone != null
                    || (fallback && (conn.micMuted || !conn.microphoneCapturePermitted()))) {
                return;
            }
            conn.microphone = conn.createMicrophoneCapture(conn.micMuted, c -> microphoneFailed(conn, c));
            conn.microphone.start();
        }
    }

    static MicrophoneCapture createMicrophoneCapture(
            NvConnection conn, boolean muted, MicrophoneCapture.FailureListener failed) {
        return new MicrophoneCapture(
                MoonBridge::sendMicrophonePcm,
                muted,
                conn.micProcessing,
                new MicRoute(conn.appContext),
                conn::noteMissingMicrophoneEffects,
                failed);
    }

    static void microphoneFailed(NvConnection conn, MicrophoneCapture capture) {
        synchronized (conn) {
            if (conn.microphone != capture) return;
            conn.microphone = null;
            conn.micMuted = true;
        }
        conn.micFailed.set(true);
        Runnable listener = conn.micNoticeListener;
        if (listener != null) listener.run();
    }

    static void stopMicrophone(NvConnection conn) {
        MicrophoneCapture capture;
        synchronized (conn) {
            capture = conn.microphone;
            conn.microphone = null;
        }
        if (capture != null) {
            capture.stop();
        }
    }

    static int audioPassthroughCodecs(boolean enabled, boolean muted, int channels) {
        if (!enabled || muted) return 0;
        int codecs = AudioPassthrough.playableCodecs(channels);
        if (codecs != 0) {
            LimeLog.info(
                    "Audio passthrough: the audio output plays "
                            + AudioPassthrough.names(codecs)
                            + " with "
                            + channels
                            + " channels");
        } else if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            LimeLog.warning("Audio passthrough: needs Android 10 or later; this stream plays Opus");
            MoonBridge.bridgeClDisplayMessage(
                    "Audio passthrough needs Android 10 or later. This stream plays Opus.");
        } else {
            LimeLog.warning(
                    "Audio passthrough: the audio output plays no passthrough format with "
                            + channels
                            + " channels; this stream plays Opus");
            MoonBridge.bridgeClDisplayMessage(
                    "This audio output plays no passthrough format. This stream plays Opus.");
        }
        return codecs;
    }
}
