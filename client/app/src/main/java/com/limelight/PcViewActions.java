package com.limelight;

import android.content.Intent;

import com.limelight.computers.HostMenu;
import com.limelight.files.FileTransferActivity;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.preferences.AddComputerManually;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.io.StringReader;
import java.util.Objects;

/**
 * Per-host action routing, tap dispatch, Add PC chooser, pairing guide, app-list opening,
 * and background running-game lookup extracted from {@link PcView}.
 */
final class PcViewActions {
    private final PcView view;

    PcViewActions(PcView view) {
        this.view = view;
    }

    /** Type an address or scan the code a host shows. A device with no camera has only the first, so + goes straight to it. */
    void offerAddPc() {
        Intent add = new Intent(view, AddComputerManually.class);
        if (!view.getPackageManager().hasSystemFeature(android.content.pm.PackageManager.FEATURE_CAMERA_ANY)) {
            view.startActivity(add);
            return;
        }
        if (view.addPcSheet != null) return;
        ActionSheet sheet = new ActionSheet(view, view.getString(R.string.title_add_pc));
        view.addPcSheet = sheet;
        sheet.addAction(R.drawable.ic_add, view.getString(R.string.title_add_pc)).setOnClickListener(v -> {
            sheet.dismiss();
            view.startActivity(add);
        });
        sheet.addAction(R.drawable.ic_glass_grid, view.getString(R.string.addpc_scan_code)).setOnClickListener(v -> {
            sheet.dismiss();
            view.startActivity(new Intent(view, AddComputerManually.class).putExtra(AddComputerManually.EXTRA_SCAN, true));
        });
        sheet.setOnDismissListener(dialog -> view.addPcSheet = null);
        sheet.show();
    }

    void showPairingGuide() {
        new NavigationDialogBuilder(view)
                .setTitle(R.string.guide_title)
                .setMessage(R.string.guide_text)
                .setPositiveButton(R.string.guide_done, null)
                .setNeutralButton(R.string.guide_troubleshooting,
                        (dialog, which) -> HelpLauncher.showHelp(view))
                .show();
    }

    void doAppList(ComputerDetails computer, boolean newlyPaired, boolean showHiddenGames) {
        if (computer.state == ComputerDetails.State.OFFLINE) {
            view.hostActions.showError(computer.displayName(), PcGridAdapter.offlineReason(view, computer,
                    view.getString(R.string.error_pc_offline)), null);
            return;
        }
        if (view.managerBinder == null) {
            view.hostActions.showError(computer.displayName(), view.getString(R.string.error_manager_not_running), null);
            return;
        }

        Intent i = new Intent(view, AppView.class);
        i.putExtra(AppView.NAME_EXTRA, computer.displayName());
        i.putExtra(AppView.UUID_EXTRA, computer.uuid);
        i.putExtra(AppView.NEW_PAIR_EXTRA, newlyPaired);
        i.putExtra(AppView.SHOW_HIDDEN_APPS_EXTRA, showHiddenGames);
        view.startActivity(i);
    }

    boolean onComputerAction(PcView.ComputerObject computer, int action) {
        switch (action) {
            case HostMenu.PAIR:
                view.hostActions.pair(computer.details);
                return true;

            case HostMenu.OTP_PAIR:
                view.hostActions.pairWithOneTimePin(computer.details);
                return true;

            case HostMenu.UNPAIR:
                view.hostActions.confirmUnpair(computer.details);
                return true;

            case HostMenu.RENAME:
                view.hostActions.rename(computer.details);
                return true;

            case HostMenu.CONNECTION_ADDRESS:
                view.hostActions.chooseAddress(computer.details);
                return true;

            case HostMenu.WAKE:
                view.doWakeOnLan(computer.details);
                return true;

            case HostMenu.SLEEP:
                view.hostActions.sleep(computer.details);
                return true;

            case HostMenu.RESTART:
                view.hostActions.pcPower(computer.details, NativePcPower.Action.RESTART);
                return true;

            case HostMenu.SHUT_DOWN:
                view.hostActions.pcPower(computer.details, NativePcPower.Action.SHUT_DOWN);
                return true;

            case HostMenu.UPDATE_HOST:
                view.hostActions.updateHost(computer.details);
                return true;

            case HostMenu.PAIR_ANOTHER:
                view.hostActions.pairAnother(computer.details);
                return true;

            case HostMenu.COOP:
                view.hostActions.joinCoop(computer.details);
                return true;

            case HostMenu.MIC_ONLY:
                view.hostActions.micOnly(computer.details);
                return true;

            case HostMenu.FILES:
                view.startActivity(FileTransferActivity.forPc(view, computer.details.uuid));
                return true;

            case HostMenu.REMOVE:
                view.hostActions.confirmRemove(computer.details);
                return true;

            case HostMenu.LIBRARY:
                doAppList(computer.details, false, false);
                return true;

            case HostMenu.RESUME:
                if (view.managerBinder == null) {
                    view.hostActions.showError(computer.details.displayName(), view.getString(R.string.error_manager_not_running), null);
                    return true;
                }
                ServerHelper.doStart(view, HostMenu.runningApp(computer.runningGame, computer.details), computer.details, view.managerBinder,
                        LaunchSource.PC_RESUME);
                return true;

            case HostMenu.QUIT:
                if (view.managerBinder == null) {
                    view.hostActions.showError(computer.details.displayName(), view.getString(R.string.error_manager_not_running), null);
                    return true;
                }
                UiHelper.displayQuitConfirmationDialog(view, () -> ServerHelper.doQuit(view, computer.details,
                        HostMenu.runningApp(computer.runningGame, computer.details), view.managerBinder, null), null);
                return true;

            case HostMenu.PC_SETTINGS:
                HostStreamSettings.show(view, computer.details.uuid, computer.details.displayName());
                return true;

            case HostMenu.SHARED_FOLDERS:
                FolderBrowserActivity.open(view, computer.details);
                return true;

            case HostMenu.PAIRING_HELP:
                showPairingGuide();
                return true;

            default:
                return false;
        }
    }

    /** A tap does the row's main action; a PC with nothing to do opens its menu. */
    void onComputerTapped(PcView.ComputerObject computer) {
        PcGridAdapter.RowState state = computer.rowState();
        if (LaunchPolicy.resumesOnPcTap(PreferenceConfiguration.readPreferences(view).quickResume,
                state == PcGridAdapter.RowState.RUNNING)) {
            onComputerAction(computer, HostMenu.RESUME);
            return;
        }
        switch (state) {
            case RUNNING:
            case READY:
                doAppList(computer.details, false, false);
                break;
            case NOT_PAIRED:
                view.hostActions.pair(computer.details);
                break;
            case WAKING:
            case OFFLINE:
                if (WakeTargets.hasMac(view, computer.details)) {
                    view.doWakeOnLan(computer.details);
                } else {
                    view.showComputerMenu(computer);
                }
                break;
            default:
                view.showComputerMenu(computer);
                break;
        }
    }

    /** Looks the running game up in the PC's app list, off the main thread, whenever it changes. */
    void findRunningGame(PcView.ComputerObject computer) {
        ComputerDetails details = computer.details;
        String key = details.hasRunningGame() ? details.runningGameId + "/" + details.runningGameUuid : null;
        if (Objects.equals(key, computer.runningGameKey)) {
            return;
        }
        computer.runningGameKey = key;
        computer.runningGame = null;
        if (key == null) {
            return;
        }
        String uuid = details.uuid;
        String rawAppList = details.rawAppList;
        HostCapabilities caps = details.caps;
        ComputerDetails running = new ComputerDetails();
        running.runningGameId = details.runningGameId;
        running.runningGameUuid = details.runningGameUuid;
        new Thread(() -> {
            NvApp game = null;
            try {
                String list = rawAppList != null ? rawAppList : CacheHelper.readInputStreamToString(
                        CacheHelper.openCacheFileForInput(view.getCacheDir(), "applist", uuid));
                game = PcGridAdapter.runningApp(NvHTTP.getAppListByReader(new StringReader(list), caps), running);
            } catch (IOException | XmlPullParserException e) {
                // No app list yet: the row says a game is running without naming it.
            }
            final NvApp found = game;
            view.runOnUiThread(() -> {
                if (found != null && key.equals(computer.runningGameKey) && !view.isFinishing() && !view.isDestroyed()) {
                    computer.runningGame = found;
                    view.pcGridAdapter.notifyDataSetChanged();
                }
            });
        }, "Running game").start();
    }
}
