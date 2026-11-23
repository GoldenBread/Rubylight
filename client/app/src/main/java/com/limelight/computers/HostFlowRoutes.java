package com.limelight.computers;

import android.app.Activity;
import android.content.Context;
import android.text.InputFilter;
import android.text.InputType;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.KnownUpdates;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;

import androidx.appcompat.app.AlertDialog;

import org.xmlpull.v1.XmlPullParserException;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.net.UnknownHostException;
import java.util.List;
import java.util.function.BiConsumer;
import java.util.function.Consumer;

/**
 * UI-thread routing and localized string translation for Sleep PC, Restart/Shut down PC,
 * Host Update, Pairing, and Unpair flows backed by {@link NightjarPolicyJni}.
 */
final class HostFlowRoutes {
    private HostFlowRoutes() {
    }

    static boolean refused(int status) {
        return NightjarPolicyJni.pairingRefusedStatus(status);
    }

    static int refusalMessage(boolean fromLink, boolean oneTimePin) {
        int code = NightjarPolicyJni.classifyPairingHttpRefusal(fromLink, oneTimePin);
        if (code == 2) return R.string.pair_qr_incorrect_pin;
        if (code == 1) return R.string.pair_otp_incorrect_pin;
        return R.string.pair_fail;
    }

    static int failureMessage(PairState state, boolean fromLink, boolean oneTimePin, boolean inGame) {
        switch (NightjarPolicyJni.classifyPairingFailure(state.ordinal(), fromLink, oneTimePin, inGame)) {
            case 0: return R.string.pair_incorrect_pin;
            case 1: return R.string.pair_otp_incorrect_pin;
            case 2: return R.string.pair_qr_incorrect_pin;
            case 3: return R.string.pair_already_in_progress;
            case 4: return R.string.pair_unverified;
            case 5: return R.string.pair_pc_ingame;
            case 7: return R.string.pair_qr_cert_mismatch;
            default: return R.string.pair_fail;
        }
    }

    static boolean alreadyPaired(PairState state, PairingLink link, java.security.cert.X509Certificate pinned) {
        return NightjarPolicyJni.alreadyPaired(state == PairState.PAIRED, link != null, pinned != null);
    }

    static int unpairErrorMessage(boolean isNotPaired, boolean isUnknownHost, boolean isNotFound) {
        switch (NightjarPolicyJni.classifyUnpair(-1, isNotPaired, isUnknownHost, isNotFound)) {
            case 0: return R.string.unpair_error;
            case 1: return R.string.error_unknown_host;
            case 2: return R.string.error_404;
            default: return R.string.unpair_fail;
        }
    }

    static int powerTitle(NativePcPower.Action action) {
        return action == NativePcPower.Action.RESTART
                ? R.string.host_menu_restart
                : R.string.host_menu_shutdown;
    }

    static String sleepUnavailableText(Context context, NativeApiClient.PowerReply offer) {
        switch (NightjarPolicyJni.sleepUnavailableReason(offer.sleepSupported, offer.sleepAllowed)) {
            case 0:
                return context.getString(R.string.host_sleep_unsupported);
            case 1:
                return context.getString(R.string.host_feature_reason_sleep_permission) + "\n\n"
                        + context.getString(R.string.host_sleep_permission_help);
            default:
                return context.getString(R.string.host_sleep_busy);
        }
    }

    static String sleepRefusedText(Context context, String message, int httpStatus) {
        return message.isEmpty() ? context.getString(R.string.host_sleep_refused, httpStatus) : message;
    }

    static boolean sleepRetries(NativeApiException error) {
        return error.getKind() == NativeApiException.Kind.NETWORK
                || error.getKind() == NativeApiException.Kind.TLS;
    }

    static String sleepFailedText(Context context, NativeApiException error) {
        if (sleepRetries(error)) {
            return context.getString(R.string.host_sleep_failed);
        }
        if (error.getKind() == NativeApiException.Kind.NOT_PAIRED) {
            return context.getString(R.string.host_feature_reason_pair);
        }
        return context.getString(R.string.host_sleep_error, error.getMessage());
    }

    static String powerUnavailableText(Context context, String reason) {
        return reason.isEmpty() ? context.getString(R.string.host_power_busy) : reason;
    }

    static String powerConfirmText(Context context, NativePcPower.Action action, boolean wakeFromOff) {
        String text = context.getString(R.string.host_power_message);
        if (action == NativePcPower.Action.SHUT_DOWN && !wakeFromOff) {
            text += "\n\n" + context.getString(R.string.host_shutdown_no_wake);
        }
        return text;
    }

    static String powerAcceptedText(Context context, NativePcPower.Action action, String name) {
        return context.getString(
                action == NativePcPower.Action.RESTART
                        ? R.string.host_restart_accepted
                        : R.string.host_shutdown_accepted,
                name);
    }

    static final class SleepRoute implements SleepFlow.Ui {
        private final Context app;
        private SleepFlow.Ui screen;
        private boolean dropped;

        SleepRoute(Context app, SleepFlow.Ui screen) {
            this.app = app;
            this.screen = screen;
        }

        void detach() {
            screen = null;
        }

        void drop() {
            dropped = true;
            screen = null;
        }

        private boolean speaks(String outcome) {
            if (dropped) {
                LimeLog.info("Sleep PC " + outcome + " after it was replaced or cancelled; not shown");
            }
            return !dropped;
        }

        private void toast(String text) {
            Toast.makeText(app, text, Toast.LENGTH_LONG).show();
        }

        @Override
        public void confirm(Runnable sleep) {
            LimeLog.info("Sleep PC offered a token");
            if (!speaks("offer")) return;
            if (screen != null) {
                screen.confirm(sleep);
            } else {
                LimeLog.info("Sleep PC not confirmed: its screen closed first");
            }
        }

        @Override
        public void accepted() {
            LimeLog.info("Sleep PC accepted");
            if (!speaks("acceptance")) return;
            if (screen != null) {
                screen.accepted();
            } else {
                toast(app.getString(R.string.host_sleep_accepted));
            }
        }

        @Override
        public void unavailable(NativeApiClient.PowerReply offer) {
            LimeLog.warning("Sleep PC offered no token: supported=" + offer.sleepSupported
                    + " allowed=" + offer.sleepAllowed + " available=" + offer.sleepAvailable);
            if (!speaks("answer")) return;
            if (screen != null) {
                screen.unavailable(offer);
            } else {
                toast(sleepUnavailableText(app, offer));
            }
        }

        @Override
        public void refused(String message, int httpStatus) {
            LimeLog.warning("Sleep PC refused: HTTP " + httpStatus + " " + message);
            if (!speaks("refusal")) return;
            if (screen != null) {
                screen.refused(message, httpStatus);
            } else {
                toast(sleepRefusedText(app, message, httpStatus));
            }
        }

        @Override
        public void failed(NativeApiException error) {
            LimeLog.warning("Sleep PC failed: " + error.getKind() + " " + error.getMessage());
            if (!speaks("failure")) return;
            if (screen != null) {
                screen.failed(error);
            } else {
                toast(sleepFailedText(app, error));
            }
        }
    }

    static class Route {
        final Context app;
        private final String what;
        private HostActions screen;
        private boolean dropped;

        Route(Context app, HostActions screen, String what) {
            this.app = app;
            this.screen = screen;
            this.what = what;
        }

        void detach() {
            screen = null;
        }

        void drop() {
            dropped = true;
        }

        void deliver(String outcome, Consumer<HostActions> shown, String toast) {
            LimeLog.info(what + " " + outcome);
            if (dropped) {
                LimeLog.info(what + " " + outcome + " after it was replaced or cancelled; not shown");
            } else if (screen != null) {
                shown.accept(screen);
            } else if (toast != null) {
                Toast.makeText(app, toast, Toast.LENGTH_LONG).show();
            }
        }
    }

    static PcPowerFlow.Ui pcPowerUi(
            final ComputerDetails computer,
            final NativePcPower.Action action,
            final Route route) {
        final Context app = route.app;
        final String title = app.getString(powerTitle(action));
        final String name = computer.displayName();
        return new PcPowerFlow.Ui() {
            @Override
            public void confirm(boolean wakeFromOff, Runnable go) {
                route.deliver("offer", screen -> {
                    screen.dismissPowerProgress();
                    screen.confirmWithCancelFocused(
                            app.getString(action == NativePcPower.Action.RESTART
                                    ? R.string.host_restart_title
                                    : R.string.host_shutdown_title, name),
                            powerConfirmText(app, action, wakeFromOff),
                            action == NativePcPower.Action.RESTART
                                    ? R.string.host_restart_action
                                    : R.string.host_shutdown_action,
                            () -> {
                                screen.showPowerProgress(powerTitle(action), true);
                                go.run();
                            });
                }, null);
            }

            @Override
            public void accepted() {
                String text = powerAcceptedText(app, action, name);
                route.deliver("acceptance", screen -> {
                    screen.dismissPowerProgress();
                    if (!screen.gone()) Toast.makeText(screen.activity(), text, Toast.LENGTH_LONG).show();
                }, text);
            }

            @Override
            public void unavailable(String reason) {
                String text = powerUnavailableText(app, reason);
                route.deliver("answer: " + reason, screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, text, null);
                }, text);
            }

            @Override
            public void refused(String message) {
                route.deliver("refusal: " + message, screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, message, null);
                }, message);
            }

            @Override
            public void failed(NativeApiException error) {
                String text = sleepFailedText(app, error);
                route.deliver("failure: " + error.getKind() + " " + error.getMessage(), screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, text,
                            sleepRetries(error) ? () -> screen.pcPower(computer, action) : null);
                }, text);
            }

            @Override
            public void refusedLater(String refusal, boolean unconfirmed) {
                boolean restart = action == NativePcPower.Action.RESTART;
                int titleId = unconfirmed
                        ? (restart ? R.string.host_restart_unconfirmed_title : R.string.host_shutdown_unconfirmed_title)
                        : (restart ? R.string.host_restart_refused_title : R.string.host_shutdown_refused_title);
                String laterTitle = app.getString(titleId, name);
                route.deliver((unconfirmed ? "unconfirmed after the answer: " : "refusal after the answer: ") + refusal,
                        screen -> {
                            screen.dismissPowerProgress();
                            screen.showError(laterTitle, refusal, null);
                        }, laterTitle + ": " + refusal);
            }
        };
    }

    static HostUpdateFlow.Ui updateUi(final ComputerDetails computer, final Route route) {
        final Context app = route.app;
        final String name = computer.displayName();
        final String uuid = computer.uuid;
        final String failedTitle = app.getString(R.string.host_update_failed_title, name);
        return new HostUpdateFlow.Ui() {
            @Override
            public void started(String version) {
                String text = app.getString(R.string.host_update_started, version);
                route.deliver("start " + version, screen -> screen.showUpdateDialog(
                        app.getString(R.string.host_update_started_title, name), text), null);
            }

            @Override
            public void waiting(String reason) {
                String text = app.getString(R.string.host_update_waiting, reason);
                route.deliver("waiting: " + reason, screen -> screen.showUpdateDialog(
                        app.getString(R.string.host_update_started_title, name), text), null);
            }

            @Override
            public void back(String version) {
                KnownUpdates.forget(uuid);
                String title = app.getString(R.string.host_update_back_title, name);
                String text = app.getString(R.string.host_update_back, version);
                route.deliver("back on " + version, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(title, text, null);
                }, title);
            }

            @Override
            public void timedOut(String version) {
                String text = app.getString(R.string.host_update_timeout, version);
                route.deliver("timeout", screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void nothingToInstall() {
                String text = app.getString(R.string.host_update_none);
                route.deliver("nothing to install", screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void updateFailed(String error) {
                String text = error != null ? error : app.getString(R.string.host_update_failed);
                route.deliver("failed on the PC: " + error, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void cameBackOn(String version, String error) {
                String back = app.getString(R.string.host_update_old_version, version);
                String text = error != null ? back + " " + error : back;
                route.deliver("back on the old version " + version, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void refused(String message) {
                route.deliver("refusal: " + message, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, message, null);
                }, message);
            }

            @Override
            public void failed(NativeApiException error) {
                String text = sleepFailedText(app, error);
                route.deliver("failure: " + error.getKind() + " " + error.getMessage(), screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }
        };
    }

    static void showPairOneTimePinDialog(
            Activity activity, ComputerDetails computer, BiConsumer<String, String> onSubmit) {
        View fields = activity.getLayoutInflater().inflate(R.layout.dialog_pair_otp, null);
        EditText pin = fields.findViewById(R.id.pair_pin);
        pin.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_VARIATION_PASSWORD);
        pin.setFilters(new InputFilter[] {new InputFilter.LengthFilter(4)});
        pin.setSaveEnabled(false);
        EditText passphrase = fields.findViewById(R.id.pair_passphrase);
        passphrase.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        passphrase.setSingleLine(true);
        passphrase.setSaveEnabled(false);
        AlertDialog dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.pair_otp_title)
                .setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.pcview_menu_pair_pc, null)
                .create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            String pinValue = pin.getText().toString();
            String passphraseValue = passphrase.getText().toString();
            if (!pinValue.matches("[0-9]{4}")) {
                pin.setError(activity.getString(R.string.pair_otp_invalid_pin));
                return;
            }
            if (passphraseValue.codePointCount(0, passphraseValue.length()) < 4) {
                passphrase.setError(activity.getString(R.string.pair_otp_invalid_passphrase));
                return;
            }
            dialog.dismiss();
            onSubmit.accept(pinValue, passphraseValue);
        }));
        dialog.setOnDismissListener(ignored -> {
            pin.getText().clear();
            passphrase.getText().clear();
        });
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        dialog.show();
    }

    static void showRenameDialog(Activity activity, ComputerDetails computer, Consumer<String> onSave) {
        View fields = activity.getLayoutInflater().inflate(R.layout.dialog_rename_host, null);
        EditText name = fields.findViewById(R.id.rename_name);
        name.setFilters(new InputFilter[] {new InputFilter.LengthFilter(HostNames.MAX_LENGTH)});
        ((com.google.android.material.textfield.TextInputLayout) fields.findViewById(R.id.rename_field))
                .setPlaceholderText(computer.name);
        name.setSaveEnabled(false);
        name.setText(computer.displayName());
        name.setSelection(name.getText().length());
        AlertDialog dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.host_rename_title)
                .setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_rename_save, (d, which) -> onSave.accept(name.getText().toString()))
                .create();
        name.setOnEditorActionListener((view, action, event) -> {
            if (action != EditorInfo.IME_ACTION_DONE) return false;
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            return true;
        });
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
                | WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
        dialog.show();
    }

    static void showChooseAddressDialog(
            Activity activity,
            ComputerDetails computer,
            Consumer<ComputerDetails.AddressTuple> onSelect) {
        List<ComputerDetails.AddressTuple> known = computer.knownAddresses();
        if (computer.pinnedAddress != null && !known.contains(computer.pinnedAddress)) {
            known.add(computer.pinnedAddress);
        }
        String[] items = new String[known.size() + 1];
        items[0] = activity.getString(R.string.host_address_automatic);
        int checked = 0;
        for (int i = 0; i < known.size(); i++) {
            items[i + 1] = known.get(i).toString();
            if (known.get(i).equals(computer.pinnedAddress)) {
                checked = i + 1;
            }
        }
        new NavigationDialogBuilder(activity)
                .setTitle(R.string.host_menu_connection_address)
                .setSingleChoiceItems(items, checked, (d, which) -> {
                    d.dismiss();
                    onSelect.accept(which == 0 ? null : known.get(which - 1));
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    static void runUnpairWorker(
            Activity activity,
            HostActions actions,
            HostActions.Callbacks callbacks,
            ComputerManagerService.ComputerManagerBinder binder,
            ComputerDetails computer) {
        String failure = null;
        boolean retry = true;
        boolean unpaired = false;
        try {
            NvHTTP httpConn = new NvHTTP(
                    com.limelight.utils.ServerHelper.getCurrentAddressFromComputer(computer),
                    computer.httpsPort, binder.getUniqueId(), computer.serverCert,
                    PlatformBinding.getCryptoProvider(activity));
            if (httpConn.getPairState() == PairingManager.PairState.PAIRED) {
                httpConn.unpair();
                if (httpConn.getPairState() == PairingManager.PairState.NOT_PAIRED) {
                    unpaired = true;
                    binder.forgetCertificate(computer.uuid);
                    binder.invalidateStateForComputer(computer.uuid);
                } else {
                    failure = activity.getString(unpairErrorMessage(false, false, false));
                }
            } else {
                failure = activity.getString(unpairErrorMessage(true, false, false));
                retry = false;
            }
        } catch (UnknownHostException e) {
            failure = activity.getString(unpairErrorMessage(false, true, false));
        } catch (FileNotFoundException e) {
            failure = activity.getString(unpairErrorMessage(false, false, true));
        } catch (XmlPullParserException | IOException e) {
            LimeLog.warning("Unpair failed: " + e);
            failure = activity.getString(unpairErrorMessage(false, false, false));
        }
        final boolean done = unpaired;
        final String message = failure;
        final boolean canRetry = retry;
        activity.runOnUiThread(() -> {
            if (done) {
                ConsoleRepository.forgetHost(activity, computer.uuid);
                if (!actions.gone()) Toast.makeText(activity, R.string.unpair_success, Toast.LENGTH_LONG).show();
            } else if (callbacks.isForeground() && !actions.gone()) {
                actions.showError(R.string.pcview_menu_unpair_pc, message, canRetry ? () -> actions.unpair(computer) : null);
            } else {
                Toast.makeText(activity, message, Toast.LENGTH_LONG).show();
            }
        });
    }

    static AlertDialog showPairingDialog(
            Activity activity,
            String oneTimePin,
            String pin,
            Runnable onCancel) {
        android.text.SpannableStringBuilder pairingText = new android.text.SpannableStringBuilder();
        if (oneTimePin == null) {
            pairingText.append(activity.getString(R.string.pair_pairing_help)).append("\n\n");
            int pinStart = pairingText.length();
            pairingText.append(pin);
            pairingText.setSpan(new android.text.style.RelativeSizeSpan(2.25f), pinStart, pairingText.length(), 0);
            pairingText.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.BOLD), pinStart, pairingText.length(), 0);
            pairingText.setSpan(new android.text.style.ForegroundColorSpan(com.google.android.material.color.MaterialColors
                    .getColor(activity, androidx.appcompat.R.attr.colorPrimary, android.graphics.Color.WHITE)),
                    pinStart, pairingText.length(), 0);
        } else {
            pairingText.append(activity.getString(R.string.pairing));
        }
        AlertDialog dialog = new NavigationDialogBuilder(activity).ownSurface()
                .setTitle(R.string.pair_pairing_title)
                .setMessage(pairingText)
                .setNegativeButton(android.R.string.cancel, (d, which) -> onCancel.run())
                .setOnCancelListener(d -> onCancel.run())
                .create();
        dialog.show();
        return dialog;
    }

    static void runPairWorker(
            Activity activity,
            HostActions actions,
            ComputerManagerService.ComputerManagerBinder binder,
            ComputerDetails computer,
            PairingLink link,
            String pin,
            String oneTimePin,
            String passphrase,
            int generation) {
        String message = null;
        boolean success = false;
        try {
            binder.waitForPollingStopped();
            NvHTTP request = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer),
                    computer.httpsPort, binder.getUniqueId(), computer.serverCert,
                    PlatformBinding.getCryptoProvider(activity.getApplicationContext()));
            if (!actions.acceptPairingRequest(generation, request)) {
                request.cancelPendingRequests();
                return;
            }
            if (alreadyPaired(request.getPairState(), link, computer.serverCert)) {
                if (link != null && !link.matches(computer.serverCert)) {
                    message = activity.getString(R.string.pair_qr_cert_mismatch);
                } else {
                    success = true;
                }
            } else {
                PairingManager pairing = request.getPairingManager();
                PairState state = link != null ? pairing.pair(request.getServerInfo(true), link)
                        : pairing.pair(request.getServerInfo(true), pin, passphrase);
                if (state == PairState.PAIRED) {
                    ComputerDetails stored = binder.getComputer(computer.uuid);
                    if (stored != null) {
                        stored.serverCert = pairing.getPairedCert();
                        binder.invalidateStateForComputer(computer.uuid);
                        success = true;
                        com.limelight.console.SetupCard.offer(activity.getApplicationContext(), stored);
                    } else {
                        message = activity.getString(R.string.pair_fail);
                    }
                } else if (state == PairState.CERT_MISMATCH) {
                    message = activity.getString(R.string.pair_qr_cert_mismatch);
                } else {
                    message = activity.getString(failureMessage(state, link != null, oneTimePin != null,
                            computer.runningGameId != 0));
                }
            }
        } catch (HostHttpResponseException e) {
            LimeLog.warning("Pairing failed: " + e.getMessage());
            message = activity.getString(refused(e.getErrorCode()) ? refusalMessage(link != null, oneTimePin != null)
                    : R.string.pair_connection_fix);
        } catch (IOException | XmlPullParserException e) {
            message = activity.getString(R.string.pair_connection_fix);
        }
        final boolean paired = success;
        final String failure = message;
        activity.runOnUiThread(() -> actions.showPairingResult(generation, paired, failure, computer, oneTimePin, link != null));
    }

    static CoopSheet.Ui coopUi(
            Activity activity,
            HostActions actions,
            HostActions.Callbacks callbacks,
            ComputerDetails computer) {
        return new CoopSheet.Ui() {
            @Override
            public void problem(CharSequence title, CharSequence message, Runnable retry) {
                actions.showError(title, message, retry);
            }

            @Override
            public void start(NvApp app) {
                ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
                if (binder == null) {
                    actions.showError(R.string.host_menu_coop, activity.getString(R.string.error_manager_not_running), null);
                    return;
                }
                ServerHelper.doStart(activity, app, computer, binder, LaunchSource.COOP_JOIN);
            }

            @Override
            public void reopen() {
                actions.joinCoop(computer);
            }
        };
    }
}
