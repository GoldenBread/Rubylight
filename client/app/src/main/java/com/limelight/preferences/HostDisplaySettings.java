package com.limelight.preferences;

import android.app.Activity;
import android.widget.Toast;
import androidx.appcompat.app.AlertDialog;

import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.R;
import com.limelight.utils.UiHelper;

/** The PC page's Display arrangement picker. Only Save commits a choice for the next stream. */
public final class HostDisplaySettings {
    private HostDisplaySettings() {
    }

    /** The PC page's Display arrangement; created separately so it can restore an unconfirmed selection after rotation. */
    static AlertDialog create(Activity activity, String hostUuid, String hostName, boolean virtualDisplay,
                              java.util.function.Consumer<Boolean> setVirtualDisplay, Runnable onChanged) {
        // Display arrangement with Virtual display folded in (DisplayArrangementChoice): Save stores the
        // arrangement here and hands the switch to the page, which saves it as this PC's own value.
        CharSequence[] labels = DisplayArrangementChoice.names(activity, true);
        int checked = DisplayArrangementChoice.choice(HostDisplayChoice.load(activity, hostUuid), virtualDisplay, true);
        return new NavigationDialogBuilder(activity)
                .setTitle(activity.getString(R.string.host_display_title, hostName))
                .setSingleChoiceItems(labels, checked, null)
                .setPositiveButton(R.string.host_display_save, (dialog, which) -> {
                    // Read the view state because the pending selection may have been restored.
                    int selected = ((AlertDialog) dialog).getListView().getCheckedItemPosition();
                    if (selected < 0 || selected >= labels.length) return;
                    HostDisplayChoice.save(activity, hostUuid, DisplayArrangementChoice.ARRANGEMENTS[selected]);
                    setVirtualDisplay.accept(DisplayArrangementChoice.virtualDisplay(selected));
                    Toast.makeText(activity, R.string.host_display_saved, Toast.LENGTH_SHORT).show();
                    if (onChanged != null) onChanged.run();
                })
                .setNeutralButton(R.string.help, null)
                .setNegativeButton(android.R.string.cancel, null)
                .create();
    }

    static void bindHelp(Activity activity, AlertDialog picker) {
        picker.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view ->
                UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.host_display_menu)
                        .setMessage(R.string.host_display_explanation)
                        .setPositiveButton(android.R.string.ok, null).create()));
    }
}
