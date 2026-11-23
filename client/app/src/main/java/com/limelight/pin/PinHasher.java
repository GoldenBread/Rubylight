/**
 * Salted PBKDF2-HMAC-SHA256 PIN hashing, device calibration and constant-time comparison backed by
 * {@code nightjar-core::pin_lock} via {@link NightjarPinJni}.
 */
package com.limelight.pin;

import com.limelight.nvstream.jni.NightjarPinJni;

public final class PinHasher {
    static final int KEY_BYTES = 32;
    static final int SALT_BYTES = 16;
    /** The iteration count is measured on this device so a check costs about this long. */
    static final long TARGET_MS = 150;
    static final int MIN_ITERATIONS = 20_000;
    static final int MAX_ITERATIONS = 600_000;

    private PinHasher() {
    }

    /** PBKDF2 with HMAC-SHA256 (RFC 8018). */
    public static byte[] pbkdf2(byte[] password, byte[] salt, int iterations, int keyBytes) {
        if (password == null || password.length == 0) {
            throw new IllegalArgumentException("empty password");
        }
        byte[] derived = NightjarPinJni.hasherPbkdf2(password, salt, iterations, keyBytes);
        if (derived == null) {
            throw new IllegalStateException("PBKDF2 failed");
        }
        return derived;
    }

    /** A new record for {@code pin} with a fresh salt. The caller still owns (and zeroes) {@code pin}. */
    public static String create(byte[] pin, int iterations) {
        String created = NightjarPinJni.hasherCreate(pin, iterations);
        if (created == null) {
            throw new IllegalArgumentException("invalid PIN or iterations");
        }
        return created;
    }

    /** Whether {@code pin} matches {@code stored}; false for anything that is not a record this class wrote. */
    public static boolean verify(String stored, byte[] pin) {
        if (stored == null || pin == null || pin.length == 0) {
            return false;
        }
        return NightjarPinJni.hasherVerify(stored, pin);
    }

    /** The iteration count that makes one check take about {@link #TARGET_MS} on this device. */
    public static int calibrate() {
        return NightjarPinJni.hasherCalibrate();
    }

    /** Scales the probe run's time to the target and keeps the result in a sane range. */
    static int scale(long probeNanos) {
        return NightjarPinJni.hasherScale(probeNanos);
    }

    static boolean constantTimeEquals(byte[] a, byte[] b) {
        return NightjarPinJni.hasherConstantTimeEquals(a, b);
    }
}
