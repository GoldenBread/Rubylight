/**
 * Continue-here dialog flow, move-watch polling ({@link NightjarSessionJni#moveWatchCreate}),
 * stream take-back, and launch confirmation prompts extracted from {@link GameConnection}.
 */
package com.limelight;

import android.app.Dialog;
import android.content.DialogInterface;
import android.os.Build;
import android.widget.Button;

import androidx.appcompat.app.AlertDialog;
import androidx.appcompat.view.ContextThemeWrapper;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.StreamMove;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.nvstream.jni.NightjarSessionJni;
import com.limelight.pin.PinGate;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.ContinueHereGate;

import java.util.function.Consumer;

final class GameMoveFlow {
    private static final long MOVE_WATCH_MS = 3000;

    private GameMoveFlow() {
    }

    static void showMovedPrompt(Game game, String device, NativeApiClient client) {
        NavigationDialogBuilder builder = new NavigationDialogBuilder(game);
        builder.setTitle(NvConnection.movedMessage(game, device))
                .setCancelable(false)
                .setPositiveButton(android.R.string.ok, (dialog, which) -> game.finish());
        if (client != null
                && !PinGate.streamLocked(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID))) {
            builder.setNegativeButton(
                    R.string.continue_here_action, (dialog, which) -> takeStreamBack(game, client));
        }
        Dialog prompt = builder.create();
        game.streamConnection.destroyMoveWatch();
        game.menuActions.showTerminalDialog(prompt);
        if (client != null) {
            long handle = NightjarSessionJni.moveWatchCreate(GameConnection.MOVE_WATCH_READS);
            if (handle != 0) {
                game.streamConnection.moveWatchHandle = handle;
                game.streamConnection.moveWatchDialog = prompt;
                watchMovedPrompt(game, prompt, client, handle);
            }
        }
    }

    private static void clearMoveWatch(Game game, long watchHandle) {
        if (game.streamConnection.moveWatchHandle == watchHandle) {
            game.streamConnection.destroyMoveWatch();
        }
    }

    private static void watchMovedPrompt(
            Game game, Dialog prompt, NativeApiClient client, long watchHandle) {
        if (watchHandle == 0 || game.streamConnection.moveWatchHandle != watchHandle) return;
        Runnable pollTask =
                () -> {
                    if (game.streamConnection.moveWatchHandle != watchHandle) return;
                    game.streamConnection.moveWatchTask = null;
                    if (!prompt.isShowing() || game.isFinishing() || game.isDestroyed()) {
                        clearMoveWatch(game, watchHandle);
                        return;
                    }
                    new Thread(
                                    () -> {
                                        StreamMove read = null;
                                        try {
                                            read = client.streamMove();
                                        } catch (NativeApiException e) {
                                            LimeLog.info(
                                                    "Could not ask the PC whether the move failed: "
                                                            + e.getMessage());
                                        }
                                        final StreamMove answer = read;
                                        game.runOnUiThread(
                                                () -> {
                                                    if (game.streamConnection.moveWatchHandle != watchHandle) {
                                                        return;
                                                    }
                                                    boolean showing =
                                                            prompt.isShowing()
                                                                    && !game.isFinishing()
                                                                    && !game.isDestroyed();
                                                    boolean movedKnown = answer != null && answer.movedTo != null;
                                                    boolean owns = answer != null && answer.ownedHere;
                                                    boolean cameBack = answer != null
                                                            && NightjarPolicyJni.continueHereCameBack(movedKnown, owns);
                                                    int step =
                                                            NightjarSessionJni.moveWatchStep(
                                                                    watchHandle,
                                                                    showing,
                                                                    answer != null,
                                                                    movedKnown,
                                                                    owns);
                                                    if (!showing) {
                                                        clearMoveWatch(game, watchHandle);
                                                        return;
                                                    }
                                                    if (step == 1 && cameBack) {
                                                        clearMoveWatch(game, watchHandle);
                                                        LimeLog.info(
                                                                "The move failed; the game is this device's again");
                                                        prompt.dismiss();
                                                        showCameBackPrompt(game, client);
                                                    } else if (answer != null && answer.movedTo == null && answer.pressFailed) {
                                                        clearMoveWatch(game, watchHandle);
                                                        LimeLog.info(
                                                                "The move failed; the game stayed with another device");
                                                        prompt.dismiss();
                                                        showMoveFailedPrompt(game, client);
                                                    } else if (answer != null && answer.movedTo == null) {
                                                        clearMoveWatch(game, watchHandle);
                                                    } else if (step == 2) {
                                                        watchMovedPrompt(
                                                                game, prompt, client, watchHandle);
                                                    } else {
                                                        clearMoveWatch(game, watchHandle);
                                                    }
                                                });
                                    },
                                    "Moved prompt")
                            .start();
                };
        game.streamConnection.moveWatchTask = pollTask;
        game.bitrateHandler.postDelayed(pollTask, MOVE_WATCH_MS);
    }

    private static void showCameBackPrompt(Game game, NativeApiClient client) {
        game.menuActions.showTerminalDialog(
                new NavigationDialogBuilder(game)
                        .setTitle(R.string.continue_here_came_back)
                        .setCancelable(false)
                        .setPositiveButton(
                                R.string.applist_menu_resume,
                                (dialog, which) -> askWhoPlaysThenResume(game, client))
                        .setNegativeButton(android.R.string.ok, (dialog, which) -> game.finish())
                        .create());
    }

    private static void showMoveFailedPrompt(Game game, NativeApiClient client) {
        game.menuActions.showTerminalDialog(
                new NavigationDialogBuilder(game)
                        .setTitle(R.string.continue_here_did_not_move)
                        .setCancelable(false)
                        .setPositiveButton(
                                R.string.continue_here_stream_too,
                                (dialog, which) -> joinOrResume(game, client))
                        .setNegativeButton(android.R.string.ok, (dialog, which) -> game.finish())
                        .create());
    }

    private static void askWhoPlaysThenResume(Game game, NativeApiClient client) {
        readWhoPlays(
                game,
                client,
                "Came back read",
                read -> {
                    if (read != null && playsThisGame(game, read)) {
                        showWhoPlaysPrompt(game, read, client);
                    } else if (read != null
                            && NightjarPolicyJni.continueHereJoinsAfterRead(
                                    false, false, read.pressFailed)) {
                        joinTheGame(game, read);
                    } else {
                        resumeTheGame(game);
                    }
                });
    }

    private static void joinOrResume(Game game, NativeApiClient client) {
        readWhoPlays(
                game,
                client,
                "Stream too read",
                read -> {
                    if (read != null
                            && NightjarPolicyJni.continueHereJoinsAfterRead(
                                    true, playsThisGame(game, read), read.pressFailed)) {
                        joinTheGame(game, read);
                    } else {
                        resumeTheGame(game);
                    }
                });
    }

    private static void readWhoPlays(
            Game game, NativeApiClient client, String thread, Consumer<StreamMove> answer) {
        new Thread(
                        () -> {
                            StreamMove read =
                                    ContinueHereGate.readWithin(
                                            client::streamMove, ContinueHereGate.SELECT_READ_WAIT_MS);
                            game.runOnUiThread(
                                    () -> {
                                        if (game.isFinishing() || game.isDestroyed()) return;
                                        if (!game.foreground) {
                                            game.streamRecovery.leftDuringPress();
                                            return;
                                        }
                                        answer.accept(read);
                                    });
                        },
                        thread)
                .start();
    }

    private static boolean playsThisGame(Game game, StreamMove read) {
        NvApp streamed = game.app;
        if (game.recoveryAppId != 0) {
            streamed = new NvApp("app", game.recoveryAppId, false);
            streamed.setAppUuid(game.recoveryAppUuid);
        }
        return read.playingOn != null
                && streamed != null
                && NightjarPolicyJni.continueHereAsksOnSelect(
                        true,
                        true,
                        streamed.matchesRunningApp(read.playingAppId, read.playingAppUuid));
    }

    private static void showWhoPlaysPrompt(Game game, StreamMove move, NativeApiClient client) {
        NavigationDialogBuilder builder = new NavigationDialogBuilder(game);
        builder.setMessage(
                        game.getString(
                                R.string.continue_here_playing_on, move.playingTitle, move.playingOn))
                .setCancelable(false)
                .setNeutralButton(
                        R.string.continue_here_stream_too,
                        (dialog, which) -> joinTheGame(game, move))
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> game.finish());
        if (!PinGate.streamLocked(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID))) {
            builder.setPositiveButton(
                    R.string.continue_here_action, (dialog, which) -> takeStreamBack(game, client));
        }
        game.menuActions.showTerminalDialog(builder.create());
    }

    private static void resumeTheGame(Game game) {
        game.streamRecovery.reconnectStream(
                PinGate.needsPinForStream(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID)),
                true);
    }

    private static void joinTheGame(Game game, StreamMove read) {
        game.streamRecovery.joinRunningGame(
                PinGate.needsPinForStream(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID)),
                read.playingAppId,
                read.playingAppUuid);
    }

    private static void takeStreamBack(Game game, NativeApiClient client) {
        new Thread(
                        () -> {
                            String failure = null;
                            try {
                                client.continueHere();
                            } catch (NativeApiException e) {
                                LimeLog.warning("Continue here failed: " + e.getMessage());
                                failure = e.getMessage();
                            }
                            String why = failure;
                            game.runOnUiThread(
                                    () -> {
                                        if (game.isFinishing() || game.isDestroyed()) return;
                                        if (!game.foreground) {
                                            game.streamRecovery.leftDuringPress();
                                            return;
                                        }
                                        if (why == null) {
                                            game.streamRecovery.reconnectStream(false, true);
                                            return;
                                        }
                                        game.menuActions.showTerminalDialog(
                                                new NavigationDialogBuilder(game)
                                                        .setTitle(R.string.continue_here_failed)
                                                        .setMessage(why)
                                                        .setCancelable(false)
                                                        .setPositiveButton(
                                                                android.R.string.ok,
                                                                (dialog, which) -> game.finish())
                                                        .create());
                                    });
                        },
                        "Continue here")
                .start();
    }

    static void launchConfirmationRequired(
            Game game, GameConnection conn, LaunchConfirmation confirmation) {
        game.runOnUiThread(
                () -> {
                    if (!game.foreground
                            || game.isFinishing()
                            || confirmation.getState(LaunchConfirmation.nowMs())
                                    != LaunchConfirmation.State.WAITING) {
                        confirmation.cancel();
                        return;
                    }
                    if (game.spinner != null) {
                        game.spinner.dismiss();
                        game.spinner = null;
                    }
                    game.pendingConfirmation = confirmation;
                    AlertDialog dialog =
                            new NavigationDialogBuilder(
                                            new ContextThemeWrapper(game, R.style.StreamTheme))
                                    .setTitle(R.string.launch_confirm_title)
                                    .setMessage(launchConfirmationMessage(game, confirmation))
                                    .setPositiveButton(
                                            R.string.launch_confirm,
                                            (d, which) -> {
                                                confirmation.confirm(LaunchConfirmation.nowMs());
                                                game.spinner =
                                                        conn.showConnecting(
                                                                game.getString(
                                                                        R.string.conn_establishing_msg));
                                            })
                                    .setNegativeButton(
                                            android.R.string.cancel,
                                            (d, which) -> {
                                                confirmation.cancel();
                                                game.finish();
                                            })
                                    .create();
                    dialog.setOnCancelListener(
                            d -> {
                                confirmation.cancel();
                                game.finish();
                            });
                    dialog.setCanceledOnTouchOutside(false);
                    dialog.show();
                    conn.launchConfirmationDialog = dialog;
                    Button cancel = dialog.getButton(DialogInterface.BUTTON_NEGATIVE);
                    if (cancel != null) {
                        if (Build.VERSION.SDK_INT >= 26) cancel.setFocusedByDefault(true);
                        cancel.requestFocus();
                    }
                });
    }

    private static String launchConfirmationMessage(Game game, LaunchConfirmation confirmation) {
        if (confirmation.isTerminate()) {
            return game.getString(R.string.launch_confirm_terminate);
        }
        String replaced = game.getIntent().getStringExtra(Game.EXTRA_REPLACED_APP_NAME);
        return replaced != null && game.appName != null
                ? game.getString(R.string.launch_confirm_replace_named, replaced, game.appName)
                : game.getString(R.string.launch_confirm_replace);
    }
}
