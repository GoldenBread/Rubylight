package com.limelight.console;

import android.content.Intent;
import android.view.View;
import android.view.ViewGroup;

import androidx.activity.ComponentActivity;

import com.limelight.AppView;
import com.limelight.FolderBrowserActivity;
import com.limelight.R;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.computers.HostMenu;
import com.limelight.files.FileTransferActivity;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.ui.ActionSheet;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.List;

/**
 * The PC capsule of Home and All games: it names the PC that answers, else the first. With one PC it opens that PC's options,
 * the same list as on the PCs page; with several, a sheet of them in glass that hangs under it.
 */
final class HostCapsule {
    /** What the capsule reads from its screen. */
    interface Screen {
        /** The PCs and games the screen shows now. */
        HomeModel model();

        ComputerDetails host(String uuid);

        /** The computer manager, or null while it is not bound. */
        ComputerManagerService.ComputerManagerBinder binder();

        HostActions actions();

        /** Shows only this PC's games on the screen ({@code uuid}), or every PC's (null), and saves the choice. */
        void setHostFilter(String uuid);
    }

    private final ComponentActivity activity;
    private final ViewGroup sheetHost;
    private final View capsule;
    private final Screen screen;
    private HostSheet sheet;
    private ActionSheet menu;

    /**
     * @param sheetHost the screen's root, which the sheet of PCs is laid over
     * @param capsule the capsule, with its name ({@code home_pc_name}) and dot ({@code home_pc_dot}) inside
     */
    HostCapsule(ComponentActivity activity, ViewGroup sheetHost, View capsule, Screen screen) {
        this.activity = activity;
        this.sheetHost = sheetHost;
        this.capsule = capsule;
        this.screen = screen;
    }

    /** The PC the capsule names: the first that answers, else the first. */
    static HomeModel.HostCard shownHost(HomeModel model) {
        HomeModel.HostCard shown = null;
        for (HomeModel.HostCard card : model.hosts) {
            if (shown == null || (card.online && !shown.online)) shown = card;
        }
        return shown;
    }

    /** Names the PC of {@code model}; hidden while there is none. */
    void show(HomeModel model) {
        HomeModel.HostCard shown = shownHost(model);
        final HomeModel.HostCard only = model.hosts.size() == 1 ? shown : null;
        HomeChrome.showHost(capsule, shown, v -> {
            if (only != null) showOptions(only);
            else showSheet();
        });
    }

    /** The PCs behind the capsule in glass, each with what it is doing; one opens its options, and Back leaves the capsule in focus. */
    void showSheet() {
        if (menu != null) return;
        if (sheet == null) sheet = HostSheet.attach(activity, sheetHost, capsule);
        if (sheet.isOpen()) return;
        HomeModel model = screen.model();
        List<HostSheet.Row> rows = new ArrayList<>();
        HomeModel.HostCard current = shownHost(model);
        for (HomeModel.HostCard card : model.hosts) {
            rows.add(HostSheet.rowOf(activity, card, screen.host(card.uuid), runningGameOf(model, card.uuid), card == current));
        }
        sheet.show(rows, this::showOptions);
    }

    /** The PC's options, the same list as on the PCs page (only for PCs this device is paired with). */
    void showOptions(HomeModel.HostCard card) {
        ComputerDetails details = screen.host(card.uuid);
        if (menu != null || details == null) return;
        String game = runningGameOf(screen.model(), card.uuid);
        ActionSheet options = HostMenu.build(activity, details, PcGridAdapter.rowState(details, false), game,
                action -> onAction(details, action), sheet -> addHostFilter(sheet, card.uuid));
        menu = options;
        options.setOnDismissListener(dialog -> menu = null);
        options.show();
    }

    /**
     * Home's own option, after Open library: this PC's games alone, or every PC's again. A PC that was filtered
     * away is not offered here, so the way back to every PC is on the one PC Home still shows.
     */
    private void addHostFilter(ActionSheet sheet, String uuid) {
        boolean onlyThis = uuid.equals(screen.model().hostFilter);
        com.google.android.material.button.MaterialButton action = sheet.addAction(R.drawable.ic_glass_pcs,
                activity.getString(onlyThis ? R.string.console_filter_all_pcs : R.string.console_filter_only_pc));
        action.setOnClickListener(v -> {
            sheet.dismiss();
            screen.setHostFilter(onlyThis ? null : uuid);
        });
    }

    /** The title of the game running on the PC, from what the screen shows; null when it does not say. */
    static String runningGameOf(HomeModel model, String hostUuid) {
        for (HomeModel.GameCard card : model.continuePlaying) {
            if (card.hostUuid.equals(hostUuid) && card.badge == HomeModel.Badge.RUNNING) return card.title;
        }
        return null;
    }

    static void openLibrary(android.app.Activity activity, String uuid, String name) {
        activity.startActivity(new Intent(activity, AppView.class)
                .putExtra(AppView.NAME_EXTRA, name)
                .putExtra(AppView.UUID_EXTRA, uuid));
    }

    private void onAction(ComputerDetails details, int action) {
        ComputerManagerService.ComputerManagerBinder binder = screen.binder();
        HostActions actions = screen.actions();
        switch (action) {
            case HostMenu.LIBRARY:
                openLibrary(activity, details.uuid, details.displayName());
                break;
            case HostMenu.RESUME:
            case HostMenu.QUIT:
                if (binder == null) {
                    actions.showError(details.displayName(), activity.getString(R.string.error_manager_not_running), null);
                } else if (action == HostMenu.RESUME) {
                    ServerHelper.doStart(activity, HostMenu.runningApp(null, details), details, binder,
                            LaunchSource.CONSOLE_RESUME);
                } else {
                    UiHelper.displayQuitConfirmationDialog(activity, () -> ServerHelper.doQuit(activity, details,
                            HostMenu.runningApp(null, details), binder, null), null);
                }
                break;
            case HostMenu.WAKE:
                actions.wake(details);
                break;
            case HostMenu.SLEEP:
                actions.sleep(details);
                break;
            case HostMenu.RESTART:
                actions.pcPower(details, NativePcPower.Action.RESTART);
                break;
            case HostMenu.SHUT_DOWN:
                actions.pcPower(details, NativePcPower.Action.SHUT_DOWN);
                break;
            case HostMenu.UPDATE_HOST:
                actions.updateHost(details);
                break;
            case HostMenu.PAIR_ANOTHER:
                actions.pairAnother(details);
                break;
            case HostMenu.COOP:
                actions.joinCoop(details);
                break;
            case HostMenu.MIC_ONLY:
                actions.micOnly(details);
                break;
            case HostMenu.FILES:
                activity.startActivity(FileTransferActivity.forPc(activity, details.uuid));
                break;
            case HostMenu.PC_SETTINGS:
                HostStreamSettings.show(activity, details.uuid, details.displayName());
                break;
            case HostMenu.CONNECTION_ADDRESS:
                actions.chooseAddress(details);
                break;
            case HostMenu.SHARED_FOLDERS:
                FolderBrowserActivity.open(activity, details);
                break;
            case HostMenu.RENAME:
                actions.rename(details);
                break;
            case HostMenu.UNPAIR:
                actions.confirmUnpair(details);
                break;
            case HostMenu.REMOVE:
                actions.confirmRemove(details);
                break;
            default:
                break;
        }
    }
}
