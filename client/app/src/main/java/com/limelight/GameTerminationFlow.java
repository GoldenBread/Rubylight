/**
 * Startup failure and stream termination evaluation, port tests, and recovery/dialog
 * routing extracted from {@link GameConnection}, calling {@link NightjarSessionJni}.
 */
package com.limelight;

import android.view.WindowManager;
import android.widget.Toast;

import com.limelight.binding.video.DecoderSetupFailure;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.PackedFallback;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeGuests;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.nvstream.jni.NightjarSessionJni;
import com.limelight.utils.NetworkGrace;
import com.limelight.utils.ServerHelper;

final class GameTerminationFlow {
    private GameTerminationFlow() {}

    static void stageFailed(Game game, GameConnection conn, final String stage,
            final int portFlags, final int errorCode) {
        LimeLog.info("Stream failed");
        conn.streamFailed = true;
        final boolean packedRefused = NightjarSessionJni.isPacked444Refused(errorCode);
        final boolean transientRecoveryFailure = !packedRefused && game.streamRecovery.isRecoveryAttempt()
                && NightjarPolicyJni.recoveryTransientStartup(stage, portFlags, errorCode);
        final int portTestResult = portFlags == 0 || transientRecoveryFailure || packedRefused
                ? MoonBridge.ML_TEST_RESULT_INCONCLUSIVE
                : MoonBridge.testClientConnectivity(ServerHelper.CONNECTION_TEST_SERVER, 443, portFlags);
        final String hostRefusal = !packedRefused && !transientRecoveryFailure
                && NightjarPolicyJni.exitRefusedInHandshake(stage) && game.conn != null
                ? game.conn.hostFailureReason() : null;
        if (hostRefusal != null) LimeLog.severe("The PC reported: " + hostRefusal);

        game.runOnUiThread(() -> {
            if (game.isFinishing() || game.isDestroyed()) return;
            final String movedTo = conn.movedAwayTo;
            if (movedTo != null) {
                if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
                game.displayedFailureDialog = true;
                conn.stopConnection();
                GameMoveFlow.showMovedPrompt(game, movedTo,
                        game.conn != null ? game.conn.nativeApiClient() : null);
                return;
            }
            if (packedRefused && game.conn != null && PackedFallback.restartsAsTheSameStart(
                    game.app == null || game.app.getControl() != NvApp.Control.NONE || game.app.isMicOnly())) {
                String why = "Full chroma refused at stream start (" + Game.terminationDescription(errorCode)
                        + (game.connectionErrorMessage != null ? "): " + game.connectionErrorMessage : ")");
                if (game.streamRecovery.restartWithoutPackedChroma(why, game.getString(PackedFallback.refusedToast(stage)))) return;
            } else if (packedRefused && game.streamRecovery.reconnectWithoutPackedChroma(
                    "Full chroma refused at stream start (" + Game.terminationDescription(errorCode)
                            + (game.connectionErrorMessage != null ? "): " + game.connectionErrorMessage : ")"),
                    game.getString(PackedFallback.refusedToast(stage)))) {
                return;
            }
            game.recoveryBudgetExhausted = false;
            if (transientRecoveryFailure && game.streamRecovery.tryAutomaticRecovery()) return;
            final boolean recoveryExhausted = transientRecoveryFailure && game.recoveryBudgetExhausted;
            if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
            if (!game.displayedFailureDialog && !game.foreground) {
                game.displayedFailureDialog = true;
                conn.stopConnection();
                game.finish();
                return;
            }
            if (!game.displayedFailureDialog && game.foreground && !game.isFinishing()) {
                game.displayedFailureDialog = true;
                LimeLog.severe(stage + " failed: " + errorCode);
                if (stage.contains("video") && game.videoSurface != null && game.videoSurface.isValid()) {
                    game.messages.alert(game.getResources().getText(R.string.video_decoder_init_failed), Toast.LENGTH_LONG);
                }
                DecoderSetupFailure decoderFailure = game.decoderRenderer == null ? null : game.decoderRenderer.getSetupFailure();
                int failureKind = NightjarSessionJni.classifyStartupFailure(
                        decoderFailure != null, stage, game.connectionErrorMessage != null, errorCode);
                boolean decoderFailed = failureKind == 0;
                String dialogText = Game.startupFailureText(
                        game, decoderFailure, stage, game.connectionErrorMessage, errorCode);
                if (hostRefusal != null && !decoderFailed) {
                    dialogText += "\n\n" + game.getResources().getString(R.string.host_failure_reason, hostRefusal);
                }
                if (portFlags != 0 && !decoderFailed) {
                    dialogText += "\n\n" + game.getResources().getString(R.string.check_ports_msg) + "\n"
                            + MoonBridge.stringifyPortFlags(portFlags, "\n");
                }
                if (portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE && portTestResult != 0) {
                    dialogText += "\n\n" + game.getResources().getString(R.string.nettest_text_blocked);
                }
                if (game.app.isSmallStreamInputOnly()) {
                    dialogText += "\n\n" + game.getString(R.string.input_only_small_stream_refused);
                }
                if (recoveryExhausted) dialogText = game.streamRecovery.gaveUpText(dialogText);
                conn.stopConnection();
                game.streamRecovery.showReconnectDialog(R.string.conn_error_title, dialogText);
            }
        });
    }

    static void connectionTerminated(Game game, GameConnection conn, final int reportedErrorCode) {
        LimeLog.info("Stream disconnected");
        game.streamTerminated = true;
        final boolean exitRequested = game.hostExitRequested;
        conn.streamFailed |= !exitRequested;
        if (reportedErrorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) {
            LimeLog.info("Connection terminated: the PC ended the stream");
        } else if (exitRequested) {
            LimeLog.info("Connection terminated after this device ended the session: "
                    + Game.terminationDescription(reportedErrorCode));
        } else {
            LimeLog.severe("Connection terminated: " + Game.terminationDescription(reportedErrorCode));
        }
        final HostLossWatch hostLoss = conn.hostLossWatch;
        final boolean hostGone = !exitRequested && game.connected && hostLoss != null
                && NightjarPolicyJni.recoveryTransportTermination(reportedErrorCode)
                && hostLoss.afterGoodbye() == HostLossWatch.GOODBYE_GONE;
        final boolean endedSession = !hostGone && !exitRequested && game.connected
                && NightjarPolicyJni.recoveryTransportTermination(reportedErrorCode)
                && game.conn != null && game.conn.hasEndedSession();
        if (endedSession) LimeLog.info("Connection terminated: the PC ended the previous session");

        boolean autoReconnect = game.prefConfig != null && game.prefConfig.autoReconnect;
        boolean videoStarted = game.streamRecovery.videoStarted();
        int[] pre = NightjarSessionJni.sessionEvaluateTermination(
                reportedErrorCode, exitRequested, hostGone, endedSession, false, false, videoStarted, autoReconnect);
        final int errorCode = pre[0];
        final boolean mayHaveMoved = !hostGone && !exitRequested
                && (reportedErrorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION
                        || NightjarPolicyJni.recoveryTransientTermination(reportedErrorCode))
                && pre[1] != 0 && game.conn != null;
        final boolean recoveryFollows = pre[2] != 0 && autoReconnect;

        final String movedTo = mayHaveMoved ? game.conn.streamMovedTo(recoveryFollows) : null;
        final NativeApiClient moveClient = movedTo != null ? game.conn.nativeApiClient() : null;
        if (movedTo != null) LimeLog.info("Connection terminated: the stream moved to " + movedTo);
        final String hostFailure = !exitRequested && NightjarPolicyJni.exitHostEndedStream(errorCode)
                && game.conn != null ? game.conn.hostFailureReason() : null;
        if (hostFailure != null) LimeLog.severe("The PC reported: " + hostFailure);
        final boolean guestEnded = NativeGuests.ended(hostFailure);

        int[] eval = NightjarSessionJni.sessionEvaluateTermination(
                reportedErrorCode, exitRequested, hostGone, endedSession, movedTo != null, guestEnded, videoStarted, autoReconnect);
        final boolean transientFailure = !guestEnded
                && NightjarPolicyJni.continueHereRecovers(
                        NightjarPolicyJni.recoveryRecoversTermination(errorCode, videoStarted), movedTo != null)
                && eval[3] != 0;
        final boolean skipPortTest = eval[4] != 0;
        final boolean keepRecoveryMediaFailure = eval[5] != 0;
        final int portFlags = MoonBridge.getPortFlagsFromTerminationErrorCode(errorCode);
        final int portTestResult = skipPortTest ? MoonBridge.ML_TEST_RESULT_INCONCLUSIVE
                : MoonBridge.testClientConnectivity(ServerHelper.CONNECTION_TEST_SERVER, 443, portFlags);

        game.runOnUiThread(() -> {
            if (game.isFinishing() || game.isDestroyed()) return;
            if (errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION && game.stoppedStreamResumeIntent != null) {
                game.stoppedStreamResumeIntent = null;
                if (movedTo != null) {
                    game.displayedFailureDialog = true;
                    GameMoveFlow.showMovedPrompt(game, movedTo, moveClient);
                    return;
                }
                game.finish();
                return;
            }
            if (keepRecoveryMediaFailure) game.getIntent().putExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE, true);
            game.recoveryBudgetExhausted = false;
            if (transientFailure && game.streamRecovery.tryAutomaticRecovery()) return;
            final boolean recoveryExhausted = transientFailure && game.recoveryBudgetExhausted;
            final NvConnection connection = game.conn;
            if (recoveryFollows && mayHaveMoved && connection != null
                    && connection.offersContinueHere() && !game.displayedFailureDialog) {
                new Thread(() -> {
                    final String late = connection.streamMovedTo(false);
                    final NativeApiClient lateClient = late != null ? connection.nativeApiClient() : null;
                    if (late != null) LimeLog.info("Connection terminated: the stream moved to " + late);
                    game.runOnUiThread(() -> {
                        if (game.isFinishing() || game.isDestroyed()) return;
                        endTerminatedStream(game, conn, errorCode, late, lateClient,
                                NightjarPolicyJni.continueHereRecovers(transientFailure, late != null),
                                recoveryExhausted && late == null, hostFailure, portFlags, portTestResult);
                    });
                }, "Stream move").start();
                return;
            }
            endTerminatedStream(game, conn, errorCode, movedTo, moveClient,
                    transientFailure, recoveryExhausted, hostFailure, portFlags, portTestResult);
        });
    }

    private static String terminationMessage(Game game, int errorCode) {
        String key = NightjarSessionJni.terminationTextKey(errorCode);
        if (key == null) key = "unexpected";
        switch (key) {
            case "host_gone": return game.getResources().getString(R.string.lost_connection);
            case "no_video_traffic": return game.getResources().getString(R.string.no_video_received_error);
            case "no_video_frame":
                MediaCodecDecoderRenderer frames = game.decoderRenderer;
                return game.getResources().getString(frames != null && frames.isPyroWaveHlgLost()
                        ? R.string.stream_hdr_pyrowave_hlg_lost : R.string.no_frame_received_error);
            case "unexpected_early": return game.getResources().getString(R.string.early_stop_error);
            case "protected_content": return game.getResources().getString(R.string.early_termination_error);
            case "frame_conversion": return game.getResources().getString(R.string.frame_conversion_error);
            case "vulkan_renderer":
                MediaCodecDecoderRenderer renderer = game.decoderRenderer;
                String failure = renderer != null ? renderer.getVulkanFailure() : null;
                return game.getResources().getString(R.string.vulkan_renderer_failed,
                        failure != null ? failure : game.getResources().getString(R.string.vulkan_failure_unknown));
            case "decoder":
                MediaCodecDecoderRenderer decoder = game.decoderRenderer;
                String reason = decoder != null ? decoder.getDecoderFailure() : null;
                return game.getResources().getString(decoder != null && decoder.decoderFailedWithVulkan()
                                ? R.string.decoder_failed_vulkan : R.string.decoder_failed,
                        reason != null ? reason : game.getResources().getString(R.string.vulkan_failure_unknown));
            default:
                String msg = game.getResources().getString(R.string.conn_terminated_msg) + "\n\n"
                        + game.getResources().getString(R.string.error_code_prefix) + " " + Game.errorCodeText(errorCode);
                if (game.prefConfig != null && NetworkGrace.mayHaveGivenUp(game.prefConfig.networkGraceSeconds, errorCode)) {
                    msg = game.getResources().getString(R.string.network_grace_gave_up,
                            game.prefConfig.networkGraceSeconds) + "\n\n" + msg;
                }
                return msg;
        }
    }

    private static void endTerminatedStream(Game game, GameConnection conn, int errorCode, String movedTo,
            NativeApiClient moveClient, boolean transientFailure, boolean recoveryExhausted,
            String hostFailure, int portFlags, int portTestResult) {
        if (!game.foreground && transientFailure) game.streamRecovery.rememberStreamForResume(false);
        game.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (game.controllerHandler != null) game.controllerHandler.stop();
        game.touchInput.setInputGrabState(false);

        if (!game.displayedFailureDialog) {
            game.displayedFailureDialog = true;
            conn.stopConnection();
            if (movedTo != null) {
                GameMoveFlow.showMovedPrompt(game, movedTo, moveClient);
                return;
            }
            if (errorCode != MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) {
                boolean portTestBlocked = portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE && portTestResult != 0;
                String message = portTestBlocked
                        ? game.getResources().getString(R.string.nettest_text_blocked)
                        : terminationMessage(game, errorCode);
                if (hostFailure != null) {
                    message += "\n\n" + game.getResources().getString(R.string.host_failure_reason, hostFailure);
                }
                if (portFlags != 0) {
                    message += "\n\n" + game.getResources().getString(R.string.check_ports_msg) + "\n"
                            + MoonBridge.stringifyPortFlags(portFlags, "\n");
                }
                game.streamRecovery.lastFailureCause = message;
                if (recoveryExhausted) message = game.streamRecovery.gaveUpText(message);
                boolean graceGaveUp = game.prefConfig != null
                        && NetworkGrace.mayHaveGivenUp(game.prefConfig.networkGraceSeconds, errorCode);
                boolean needsAction = game.stoppedStreamResumeIntent != null
                        || hostFailure != null
                        || NightjarPolicyJni.exitTerminationNeedsAction(
                                errorCode, portTestBlocked, graceGaveUp, recoveryExhausted);
                if (game.foreground && needsAction) {
                    game.streamRecovery.showReconnectDialog(R.string.conn_terminated_title, message);
                } else if (game.foreground) {
                    Toast.makeText(game.getApplicationContext(),
                            errorCode == MoonBridge.CLIENT_ERROR_HOST_GONE
                                    ? game.getString(R.string.lost_connection)
                                    : game.getString(R.string.conn_terminated_title) + "\n"
                                            + game.getString(R.string.error_code_prefix) + " "
                                            + Game.errorCodeText(errorCode),
                            Toast.LENGTH_LONG).show();
                    game.finish();
                } else if (game.stoppedStreamResumeIntent == null) {
                    game.finish();
                }
            } else {
                game.finish();
            }
        }
    }
}
