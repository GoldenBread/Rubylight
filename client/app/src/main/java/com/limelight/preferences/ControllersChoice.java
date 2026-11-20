package com.limelight.preferences;

import android.content.SharedPreferences;

/**
 * Controllers as one choice: Off, One player (every controller as player 1) or A player each. Stored in two
 * switches: {@code checkbox_send_controllers} (new, on by default) and {@code checkbox_multi_controller}, the
 * older "each controller its own player" switch, so a store from before reads as it streamed: its switch off
 * reads as One player. Off leaves the player switch as it was, so turning controllers back on returns to it.
 * Desktop Nightjar's row writes {@code controllers} and {@code multi_controller} the same way.
 */
public final class ControllersChoice {
    static final int OFF = 0;
    static final int ONE_PLAYER = 1;
    static final int PLAYER_EACH = 2;

    private ControllersChoice() { }

    static int read(SharedPreferences prefs) {
        if (!prefs.getBoolean(PreferenceConfiguration.SEND_CONTROLLERS_PREF_STRING, true)) return OFF;
        return prefs.getBoolean(PreferenceConfiguration.MULTI_CONTROLLER_PREF_STRING, true) ? PLAYER_EACH : ONE_PLAYER;
    }

    static void save(SharedPreferences.Editor editor, int choice) {
        editor.putBoolean(PreferenceConfiguration.SEND_CONTROLLERS_PREF_STRING, choice != OFF);
        if (choice != OFF) editor.putBoolean(PreferenceConfiguration.MULTI_CONTROLLER_PREF_STRING, choice == PLAYER_EACH);
    }

    /**
     * Whether a stream's start says that the {@code connected} controllers on this device stay here
     * (nightjar_policy::controllers::Controllers::leaves_connected_here). Only Off leaves one out: One
     * player and A player each send every controller, also one connected after the launch counted them.
     */
    public static boolean leavesConnectedHere(boolean sendControllers, int connected) {
        return !sendControllers && connected > 0;
    }

    /**
     * The notice that Controllers is Off while a controller is connected, said once a stream: at its
     * start, else for the first controller connected during it. Never on a second screen, whose
     * controllers belong to the first screen's stream.
     */
    public static final class OffNotice {
        private final boolean sendControllers;
        private final boolean secondScreen;
        private boolean said;

        public OffNotice(boolean sendControllers, boolean secondScreen) {
            this.sendControllers = sendControllers;
            this.secondScreen = secondScreen;
        }

        /** True the one time the notice is due, with {@code connected} controllers on this device. */
        public boolean take(int connected) {
            if (said || secondScreen || !leavesConnectedHere(sendControllers, connected)) return false;
            said = true;
            return true;
        }
    }

    /** The choice's name in logs, as desktop Nightjar's (Controllers::key). */
    public static String key(boolean sendControllers, boolean multiController) {
        if (!sendControllers) return "off";
        return multiController ? "player_each" : "one_player";
    }
}
