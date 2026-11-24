package com.limelight.binding.input;

import com.limelight.nvstream.jni.NightjarInputJni;

/** Which host vibration requests reach a motor, and how strongly this device vibrates in a controller's place. */
public final class RumblePolicy {
    private RumblePolicy() {}

    /**
     * "Disable rumble" silences every motor the host can drive: controller and trigger rumble,
     * waveform haptics and this device's vibration standing in for a controller.
     */
    public static boolean allows(boolean rumbleDisabled) {
        return NightjarInputJni.rumbleAllows(rumbleDisabled);
    }

    /**
     * How the host's motor levels are reshaped for small motors (the "Rumble feel" setting). Rust's
     * {@code nightjar_policy::rumble::RumbleFeel} is the desktop twin; both check the same table.
     */
    public enum Feel {
        /** The host's levels as they are. */
        STANDARD("standard"),
        /** Weak levels are lifted above a small motor's start threshold. */
        STRONG("strong"),
        /** Strong levels are capped and weak ones softened. */
        SUBTLE("subtle");

        public final String key;

        Feel(String key) {
            this.key = key;
        }

        /** The feel stored under this key; anything unknown is Standard. */
        public static Feel fromKey(String key) {
            int ord = NightjarInputJni.rumbleFeelFromKey(key);
            Feel[] all = values();
            return (ord >= 0 && ord < all.length) ? all[ord] : STANDARD;
        }
    }

    /** An unsigned 16-bit motor level after the feel's curve; zero stays zero. */
    public static short shapeMotor(short motor, Feel feel) {
        return NightjarInputJni.rumbleShapeMotor(motor, feel.ordinal());
    }

    /** A 0-255 vibration amplitude after the feel's curve. */
    public static int shapeAmplitude(int amplitude, Feel feel) {
        return NightjarInputJni.rumbleShapeAmplitude(amplitude, feel.ordinal());
    }

    /** Scales an unsigned 16-bit motor level by a percentage, capped at full strength. */
    public static short scaleMotor(short motor, int strengthPercent) {
        return NightjarInputJni.rumbleScaleMotor(motor, strengthPercent);
    }

    /**
     * The one amplitude (0-255) for a device with a single motor: each motor scaled by the
     * strength percent (capped at full), then 80% of the big motor and 33% of the small one, then
     * the feel's curve. Standard is the mix alone, so strength still sets it at every percent.
     */
    public static int singleMotorAmplitude(short lowFreqMotor, short highFreqMotor, Feel feel, int strengthPercent) {
        return NightjarInputJni.rumbleSingleMotorAmplitude(lowFreqMotor, highFreqMotor, feel.ordinal(), strengthPercent);
    }

    /** The PWM period a device without amplitude control is pulsed over, in milliseconds. */
    public static final long PWM_PERIOD_MS = 20;

    /**
     * {on, off} milliseconds for a vibrator that only switches on and off: the amplitude as the
     * share of each period it runs. A non-Standard feel keeps a weak level at 1 ms on, so the
     * floor it lifts weak rumble to is not lost to rounding.
     */
    public static long[] pwmTimings(int amplitude, Feel feel) {
        long[] out = new long[2];
        NightjarInputJni.rumblePwmTimings(amplitude, feel.ordinal(), out);
        return out;
    }
}
