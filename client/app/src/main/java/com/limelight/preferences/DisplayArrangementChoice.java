package com.limelight.preferences;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.R;

/**
 * Display arrangement with the Virtual display switch folded in, as on desktop Nightjar (settings.rs DISPLAY_CHOICES):
 * the PC's own setting, a virtual display in the PC's own arrangement, then the arrangements. Each choice is saved in
 * the arrangement ({@link HostDisplayChoice}); the PC's own setting and Virtual display also set the switch
 * ({@code checkbox_virtual_display}), which the arrangements leave as it is. So presets, per-PC values and older
 * versions keep working, and Remote Monitor, which keeps the PC's arrangement and follows the switch alone, asks for
 * what it asked for before.
 */
final class DisplayArrangementChoice {
    static final String VIRTUAL_DISPLAY_PREF = "checkbox_virtual_display";
    static final int PC_OWN = 0;
    static final int VIRTUAL_DISPLAY = 1;
    /** Each choice's arrangement, in the order the PC page's picker offered them; independent of stored names. */
    static final HostDisplayChoice[] ARRANGEMENTS = {
            HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.EXCLUSIVE,
            HostDisplayChoice.EXTENDED, HostDisplayChoice.EXTENDED_PRIMARY, HostDisplayChoice.EXTENDED_ISOLATED,
            HostDisplayChoice.EXTENDED_PRIMARY_ISOLATED, HostDisplayChoice.PHYSICAL};

    private DisplayArrangementChoice() { }

    /**
     * The choice the two stored values make. An arrangement wins, as at launch, where it asks for a virtual display by
     * itself (HostDisplayChoice.requestsVirtualDisplay); the PC's own setting shows the switch. A PC without display
     * modes reads the switch alone, as its launch does. A saved pair stays as it is until a choice is made, so no
     * stream changes by itself.
     */
    static int choice(HostDisplayChoice arrangement, boolean virtualDisplay, boolean displayModes) {
        if (displayModes && arrangement != HostDisplayChoice.HOST_DEFAULT) {
            for (int i = VIRTUAL_DISPLAY + 1; i < ARRANGEMENTS.length; i++) {
                if (ARRANGEMENTS[i] == arrangement) return i;
            }
        }
        return virtualDisplay ? VIRTUAL_DISPLAY : PC_OWN;
    }

    /** Whether a stream with this choice has a virtual display on a PC with display modes. */
    static boolean virtualDisplay(int choice) {
        return choice != PC_OWN && ARRANGEMENTS[choice] != HostDisplayChoice.PHYSICAL;
    }

    /** What the choice saves in the switch: the PC's own setting turns it off, Virtual display on; an arrangement leaves it. */
    static Boolean switchFor(int choice) {
        return choice == PC_OWN ? Boolean.FALSE : choice == VIRTUAL_DISPLAY ? Boolean.TRUE : null;
    }

    /** The choices' names; without display modes, only the PC's own setting and Virtual display. */
    static CharSequence[] names(Context context, boolean displayModes) {
        CharSequence[] names = new CharSequence[displayModes ? ARRANGEMENTS.length : VIRTUAL_DISPLAY + 1];
        for (int i = 0; i < names.length; i++) {
            names[i] = context.getString(i == VIRTUAL_DISPLAY ? R.string.title_virtual_display : ARRANGEMENTS[i].label);
        }
        return names;
    }

    /** The main settings' choice. */
    static int readMain(SharedPreferences prefs) {
        return choice(HostDisplayChoice.fromStored(prefs.getString(HostDisplayChoice.MAIN_PREF, null)),
                prefs.getBoolean(VIRTUAL_DISPLAY_PREF, false), true);
    }

    static void saveMain(SharedPreferences.Editor editor, int choice) {
        HostDisplayChoice arrangement = ARRANGEMENTS[choice];
        if (arrangement == HostDisplayChoice.HOST_DEFAULT) editor.remove(HostDisplayChoice.MAIN_PREF);
        else editor.putString(HostDisplayChoice.MAIN_PREF, arrangement.name());
        Boolean virtualDisplay = switchFor(choice);
        if (virtualDisplay != null) editor.putBoolean(VIRTUAL_DISPLAY_PREF, virtualDisplay);
    }
}
