package com.limelight.preferences;

import android.content.Context;
import android.content.pm.PackageManager;
import android.hardware.input.InputManager;
import android.os.Build;
import android.view.InputDevice;

import java.util.Locale;

/**
 * What this device has that some settings act on, for the rows Settings shows only where they can do something
 * (SettingRules.whenApplies). Asked each time the rows are placed: an attached mouse or keyboard shows its rows at once.
 * Where Android cannot tell, the answer is yes, so a row stays rather than hiding on a guess.
 */
interface SettingDevices {
    /** A physical touchpad (a laptop's or keyboard cover's, or a controller's): SOURCE_TOUCHPAD, as TrackpadContext reads it. */
    boolean touchpad();

    /** A mouse: its speed and Back and Forward buttons (GameTouchInput, GameKeyboardInput). */
    boolean mouse();

    /** A physical keyboard whose Back key KeyboardTranslator.isKeyboardBack takes. */
    boolean keyboard();

    /** A hinge (Android 11 and later): its panels are what Ready the decoder for folding sizes for (GameVideoOutput.coveringStreamSize). */
    boolean folds();

    /**
     * A MediaTek or Samsung Exynos chip, whose decoders Chipset tuning sets (MediaCodecHelper's c2.mtk and exynos
     * prefixes). Read from the chip's maker, never by listing the decoders: that is a blocking call to the media
     * service. Yes where the maker is unknown.
     */
    boolean tunedChip();

    /**
     * A touch screen, which the touch modes, the on-screen controller and an external display's trackpad all take
     * their touches from; it cannot come or go during a stream.
     */
    boolean touchscreen();

    /** This device as Android reports it now. */
    static SettingDevices of(Context context) {
        Context app = context.getApplicationContext();
        return new SettingDevices() {
            @Override public boolean touchscreen() {
                return app.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN);
            }

            @Override public boolean touchpad() {
                return any(app, device -> device.supportsSource(InputDevice.SOURCE_TOUCHPAD));
            }

            @Override public boolean mouse() {
                return any(app, device -> device.supportsSource(InputDevice.SOURCE_MOUSE) ||
                        device.supportsSource(InputDevice.SOURCE_MOUSE_RELATIVE));
            }

            @Override public boolean keyboard() {
                int gameController = InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_JOYSTICK;
                return any(app, device -> device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC &&
                        device.supportsSource(InputDevice.SOURCE_KEYBOARD) && (device.getSources() & gameController) == 0);
            }

            @Override public boolean folds() {
                return Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
                        app.getPackageManager().hasSystemFeature(PackageManager.FEATURE_SENSOR_HINGE_ANGLE);
            }

            @Override public boolean tunedChip() {
                return SettingDevices.tunedChip(Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ? Build.SOC_MANUFACTURER : null,
                        Build.HARDWARE);
            }
        };
    }

    /**
     * Whether the chip is MediaTek or Exynos, from its maker (Android 12 and later) or else the hardware name
     * (mt6893, exynos2100, universal...). Only a maker known to be neither (Qualcomm, NVIDIA) answers no: Google's
     * Tensor chips may name their decoders c2.exynos, which the tuning matches (MediaCodecHelper), and an unknown
     * maker may be one of them.
     */
    static boolean tunedChip(String socManufacturer, String hardware) {
        String maker = socManufacturer == null ? "" : socManufacturer.toLowerCase(Locale.ROOT);
        if (!maker.isEmpty() && !maker.equals("unknown")) {
            return !(maker.equals("qti") || maker.contains("qualcomm") || maker.contains("nvidia"));
        }
        String name = hardware == null ? "" : hardware.toLowerCase(Locale.ROOT);
        if (name.startsWith("mt") || name.contains("exynos") || name.startsWith("universal") || name.startsWith("s5e")) {
            return true;
        }
        // Before Android 12 a Snapdragon names itself (qcom); anything else may still be one of them.
        return !(name.startsWith("qcom") || name.contains("qualcomm") || name.equals("ranchu") || name.startsWith("tegra"));
    }

    /** Whether any non-virtual input device matches. */
    static boolean any(Context context, java.util.function.Predicate<InputDevice> matches) {
        InputManager input = (InputManager) context.getSystemService(Context.INPUT_SERVICE);
        if (input == null) return true;
        for (int id : input.getInputDeviceIds()) {
            InputDevice device = input.getInputDevice(id);
            if (device != null && !device.isVirtual() && matches.test(device)) return true;
        }
        return false;
    }
}
