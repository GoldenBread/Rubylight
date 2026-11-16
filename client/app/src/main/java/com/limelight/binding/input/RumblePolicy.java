package com.limelight.binding.input;

/** Which host vibration requests reach a motor, and how strongly this device vibrates in a controller's place. */
public final class RumblePolicy {
    private RumblePolicy() {}

    /**
     * "Disable rumble" silences every motor the host can drive: controller and trigger rumble,
     * waveform haptics and this device's vibration standing in for a controller.
     */
    public static boolean allows(boolean rumbleDisabled) {
        return !rumbleDisabled;
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
            for (Feel feel : values()) {
                if (feel.key.equals(key)) return feel;
            }
            return STANDARD;
        }
    }

    // Strong: levels under this share of full strength stay silent; every other level starts at the floor.
    private static final double STRONG_DEAD = 0.02;
    private static final double STRONG_FLOOR = 0.25;
    private static final double STRONG_GAMMA = 0.7;
    // Subtle: the strongest level reaches this share of full strength.
    private static final double SUBTLE_CAP = 0.6;
    private static final double SUBTLE_GAMMA = 1.5;

    /** An unsigned 16-bit motor level after the feel's curve; zero stays zero. */
    public static short shapeMotor(short motor, Feel feel) {
        int level = motor & 0xffff;
        if (level == 0 || feel == Feel.STANDARD) return motor;
        double f = level / 65535.0;
        double y;
        if (feel == Feel.STRONG) {
            y = f < STRONG_DEAD ? 0.0 : STRONG_FLOOR + (1.0 - STRONG_FLOOR) * Math.pow(f, STRONG_GAMMA);
        } else {
            y = SUBTLE_CAP * Math.pow(f, SUBTLE_GAMMA);
        }
        return (short) Math.max(0, Math.min(65535, Math.round(y * 65535.0)));
    }

    /** A 0-255 vibration amplitude after the feel's curve. */
    public static int shapeAmplitude(int amplitude, Feel feel) {
        if (amplitude <= 0 || feel == Feel.STANDARD) return amplitude;
        return Math.min(255, (shapeMotor((short) (amplitude * 257), feel) & 0xffff) >> 8);
    }

    /** Scales an unsigned 16-bit motor level by a percentage, capped at full strength. */
    public static short scaleMotor(short motor, int strengthPercent) {
        int scaled = ((motor & 0xffff) * Math.max(0, strengthPercent)) / 100;
        return (short) Math.min(scaled, Short.MAX_VALUE * 2);
    }

    /**
     * The one amplitude (0-255) for a device with a single motor: each motor scaled by the
     * strength percent (capped at full), then 80% of the big motor and 33% of the small one, then
     * the feel's curve. Standard is the mix alone, so strength still sets it at every percent.
     */
    public static int singleMotorAmplitude(short lowFreqMotor, short highFreqMotor, Feel feel, int strengthPercent) {
        int low = (scaleMotor(lowFreqMotor, strengthPercent) >> 8) & 0xFF;
        int high = (scaleMotor(highFreqMotor, strengthPercent) >> 8) & 0xFF;
        int mixed = Math.min(255, (int) ((low * 0.80) + (high * 0.33)));
        return shapeAmplitude(mixed, feel);
    }

    /** The PWM period a device without amplitude control is pulsed over, in milliseconds. */
    public static final long PWM_PERIOD_MS = 20;

    /**
     * {on, off} milliseconds for a vibrator that only switches on and off: the amplitude as the
     * share of each period it runs. A non-Standard feel keeps a weak level at 1 ms on, so the
     * floor it lifts weak rumble to is not lost to rounding.
     */
    public static long[] pwmTimings(int amplitude, Feel feel) {
        long on = (long) ((amplitude / 255.0) * PWM_PERIOD_MS);
        if (feel != Feel.STANDARD && amplitude > 0) on = Math.max(1, on);
        return new long[] {on, PWM_PERIOD_MS - on};
    }
}
