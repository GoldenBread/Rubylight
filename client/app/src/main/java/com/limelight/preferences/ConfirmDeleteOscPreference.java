package com.limelight.preferences;

import android.os.Bundle;
import com.limelight.ui.NavigationDialogBuilder;
import android.content.Context;
import android.content.DialogInterface;
import android.os.Build;
import android.preference.DialogPreference;
import android.util.AttributeSet;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.utils.UiHelper;

public class ConfirmDeleteOscPreference extends DialogPreference {
    public ConfirmDeleteOscPreference(Context context, AttributeSet attrs, int defStyleAttr, int defStyleRes) {
        super(context, attrs, defStyleAttr, defStyleRes);
    }

    public ConfirmDeleteOscPreference(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    public ConfirmDeleteOscPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public ConfirmDeleteOscPreference(Context context) {
        super(context);
    }

    @Override
    protected void showDialog(Bundle state) {
        androidx.appcompat.app.AlertDialog dialog = new NavigationDialogBuilder(getContext()).setTitle(getDialogTitle())
                .setMessage(getDialogMessage()).setPositiveButton(getPositiveButtonText(), this)
                .setNegativeButton(getNegativeButtonText(), null).create();
        UiHelper.showDialog(getContext(), dialog);
        UiHelper.markDestructive(dialog);
    }

    public void onClick(DialogInterface dialog, int which) {
        if (which == DialogInterface.BUTTON_POSITIVE) {
            // The controller's layouts, for all games and each game; the classic controller's old layout goes too, so
            // nothing of it is carried over later.
            getContext().getSharedPreferences(com.limelight.binding.input.pad.ClassicLayoutMigration.CLASSIC_PREFS,
                    Context.MODE_PRIVATE).edit().clear().apply();
            com.limelight.binding.input.pad.ModernController.clearLayouts(getContext());
            Toast.makeText(getContext(), R.string.toast_reset_osc_success, Toast.LENGTH_SHORT).show();
        }
    }
}
