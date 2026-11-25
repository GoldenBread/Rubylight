package com.limelight;

import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;

import com.limelight.binding.video.StagedStreamExit;

/**
 * Staged Android TV HDR stream exit controller extracted from {@link Game}: stops the decoder
 * and releases the video surface before restoring the display mode so the HDR-to-SDR switch and
 * display-mode revert never overlap on HDMI.
 */
final class GameStagedExit {
    private final Game game;
    private final Handler exitHandler = new Handler(Looper.getMainLooper());
    private final Runnable stagedExitRunnable = this::stagedExitTick;
    private boolean stagedExitStopDone;

    GameStagedExit(Game game) {
        this.game = game;
    }

    /** Returns true when {@link Game#finish()} should call {@code super.finish()} immediately. */
    boolean finish() {
        // Failures say so first; a hand-off to a new stream goes through finishNow.
        if (!game.exitFinished && !game.exiting) game.streamConnection.playerLeft();
        if (game.exitFinished) {
            return true;
        }
        if (game.exiting) {
            LimeLog.info("Exit: already in progress, ignoring finish()");
            return false;
        }
        if (stagedExitApplies()) {
            beginStagedExit();
            return false;
        }
        game.exitFinished = true;
        return true;
    }

    void onDestroy() {
        // A staged exit still waiting on the display ends here with the activity.
        exitHandler.removeCallbacksAndMessages(null);
        game.exitFinished = true;
    }

    private boolean stagedExitApplies() {
        return !game.isDestroyed() && game.attemptedConnection && game.decoderRenderer != null &&
                StagedStreamExit.applies(game.streamConnection.isAndroidTv(),
                        game.streamHdr || Boolean.TRUE.equals(game.decoderRenderer.getHostHdrReport()));
    }

    private void beginStagedExit() {
        if (!game.stagedExit.begin(SystemClock.uptimeMillis())) return;
        game.exiting = true;
        LimeLog.info("Exit: staged (Android TV, HDR stream, Experimental)");
        game.touchInput.setInputGrabState(false);
        if (game.streamMenu != null) game.streamMenu.dismiss();
        LimeLog.info("Exit: stopping the connection and releasing the decoder");
        game.streamConnection.stopConnection();
        exitHandler.post(stagedExitRunnable);
    }

    private void stagedExitTick() {
        if (game.exitFinished || game.isDestroyed()) return;
        long now = SystemClock.uptimeMillis();
        Thread stopping = game.connectionStopThread;
        if ((stopping == null || !stopping.isAlive()) && !stagedExitStopDone) {
            stagedExitStopDone = true;
            game.stagedExit.onStopFinished();
            if (game.stagedExit.stopTimedOut()) LimeLog.info("Exit: connection stop finished late");
        }
        switch (game.stagedExit.poll(now)) {
            case RELEASE_OUTPUT:
                LimeLog.info(game.stagedExit.stopTimedOut() ?
                        "Exit: connection stop still running after " + StagedStreamExit.STOP_TIMEOUT_MS +
                                " ms, releasing the video output anyway and waiting for it before the mode restore" :
                        "Exit: decoder released, releasing the video output");
                releaseVideoOutput();
                break;
            case RESTORE_DISPLAY_MODE:
                LimeLog.info("Exit: display settled, restoring the display mode");
                restoreDisplayMode();
                break;
            case FINISH:
                LimeLog.info("Exit: display settled, finishing");
                game.exitFinished = true;
                game.finishSuper();
                return;
            default:
                break;
        }
        exitHandler.postDelayed(stagedExitRunnable, StagedStreamExit.POLL_MS);
    }

    // Tears the video surface down, which ends the HDR output. The stop thread has released
    // the decoder by now (or timed out; the surface callbacks then stop it as they always do).
    private void releaseVideoOutput() {
        // Where the display is now stays requested until RESTORE_DISPLAY_MODE: on Android 12+ the
        // refresh rate may have been switched by the surface's frame-rate vote, which would
        // otherwise be dropped together with the HDR output.
        try {
            Window window = game.videoOutput.getVideoWindow();
            WindowManager.LayoutParams attributes = window.getAttributes();
            attributes.preferredDisplayModeId = game.videoOutput.getVideoDisplay().getMode().getModeId();
            window.setAttributes(attributes);
        } catch (RuntimeException e) {
            LimeLog.warning("Exit: could not pin the display mode: " + e);
        }
        if (game.streamView != null) game.streamView.setVisibility(View.GONE);
        if (game.renderView != null && game.renderView != game.streamView) game.renderView.setVisibility(View.GONE);
    }

    // Gives back what prepareDisplayForRendering() and the pin above asked of the display, instead
    // of leaving it to the window going away at the same time as something else. The surface is
    // gone by now, so its frame-rate vote went with it.
    private void restoreDisplayMode() {
        try {
            Window window = game.videoOutput.getVideoWindow();
            WindowManager.LayoutParams attributes = window.getAttributes();
            attributes.preferredDisplayModeId = 0;
            attributes.preferredRefreshRate = 0;
            window.setAttributes(attributes);
        } catch (RuntimeException e) {
            LimeLog.warning("Exit: could not restore the display mode: " + e);
        }
    }
}
