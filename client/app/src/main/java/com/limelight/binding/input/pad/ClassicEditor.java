package com.limelight.binding.input.pad;

/**
 * What the classic layout editor leaves on screen. Moving the classic controls needs them visible, but only while
 * their switch is on; a gamepad that auto-hides them may show them for the edit, and they go away again when the edit
 * ends. Plain policy, so the choices are tested without a stream.
 */
public final class ClassicEditor {
    private ClassicEditor() {
    }

    /** The controls the dialog shows and hides, so the policy needs no stream or view. */
    public interface Controls {
        /** The Show on-screen controls setting. */
        boolean switchOn();

        /** Whether the controls belong on screen now: the switch is on and no controller hides them. */
        boolean belongOnScreen();

        void show();

        void hide();
    }

    /** The dialog opens: the controls appear for editing only while their switch is on. */
    public static void open(Controls controls) {
        if (controls.switchOn()) controls.show();
        else controls.hide();
    }

    /**
     * A mode was chosen; Active ends the edit. The controls stay only if they belong on screen, so a gamepad's
     * auto-hide takes them away again instead of leaving them on top of the game.
     */
    public static void choose(Controls controls, boolean active) {
        if (!controls.switchOn()) {
            controls.hide();
            return;
        }
        controls.show();
        if (active && !controls.belongOnScreen()) controls.hide();
    }

    /** The dialog was cancelled: nothing stays on screen that does not belong there. */
    public static void cancel(Controls controls) {
        if (!controls.switchOn() || !controls.belongOnScreen()) controls.hide();
    }
}
