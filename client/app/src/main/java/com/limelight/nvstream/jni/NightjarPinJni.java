/**
 * Pure-JDK JNI declarations for {@code nightjar-policy::pin} and {@code nightjar-core::pin_lock}
 * ({@code protocol/android/src/pin.rs}). Imports only {@code java.*} so Robolectric loads it without
 * Android shadow acquisition; loads {@code nightjar_core} via {@link NightjarJni#ensureLibraryLoaded()}.
 */
package com.limelight.nvstream.jni;

public final class NightjarPinJni {
    static {
        NightjarJni.ensureLibraryLoaded();
    }

    private NightjarPinJni() {
    }

    /** Verification callback passed to {@link #storeTryPin}; exceptions propagate without clearing fail count. */
    public interface PinVerifier {
        boolean matches(String storedHash, byte[] pin);
    }

    // PinHasher
    public static native byte[] hasherPbkdf2(byte[] password, byte[] salt, int iterations, int keyBytes);
    public static native String hasherCreate(byte[] pin, int iterations);
    public static native boolean hasherVerify(String stored, byte[] pin);
    public static native int hasherCalibrate();
    public static native int hasherScale(long probeNanos);
    public static native boolean hasherConstantTimeEquals(byte[] a, byte[] b);

    // PinThrottle
    public static native long throttleDelayAfter(int fails);
    public static native long[] throttleOnBoot(int fails, long lockUntil, long lockBoot, long now, long boot);
    public static native long throttleRemainingMs(long lockUntil, long now);
    public static native long[] throttleFailed(int fails, long now, long boot);

    // PinGate pure + stateful
    public static native boolean gateAppNeeded(boolean pinSet, String mode, boolean unlocked);
    public static native boolean gateStreamNeeded(boolean pinSet, String mode, String pcsJson,
                                                  String uuid, boolean unlocked, boolean grantValid);
    public static native boolean gateContains(String pcsJson, String uuid);
    public static native boolean gateWithinGrace(boolean sameBoot, long leftAt, long now, int graceMinutes);
    public static native void gateInstall(long storeHandle, long elapsed, long boot);
    public static native boolean gateNeedsPinForApp(long storeHandle);
    public static native boolean gateNeedsPinForStream(long storeHandle, String uuid, long elapsed);
    public static native boolean gateStreamLocked(long storeHandle, String uuid, long elapsed);
    public static native void gateGrant(String uuid, long elapsed);
    public static native void gateForgetGrant();
    public static native void gateUnlockApp();
    public static native void gateFollowFirstScreen();
    public static native void gateLeft(long storeHandle, long elapsed, long boot);
    public static native void gateReturned(long storeHandle, long elapsed);
    public static native void gateResetForTests();

    // PinStore
    public static native long storeOpen(String dir, boolean cached);
    public static native void storeDestroy(long handle);
    public static native void storeFlush(long handle);
    public static native void storeEvictCacheForTests(String dir);
    public static native int storeImportLegacy(String dir, String legacyJson);
    public static native String storeViewJson(long handle);
    public static native void storeSetMode(long handle, String mode);
    public static native void storeSetPcs(long handle, String pcsJson);
    public static native void storeSetGraceMinutes(long handle, int minutes);
    public static native String storeSetPin(long handle, byte[] pin, int iterations);
    public static native void storeRemovePin(long handle);
    public static native long storeLockedForMs(long handle, long elapsed, long boot);
    public static native long[] storeTryPin(long handle, byte[] pin, long elapsed, long boot,
                                            PinVerifier verifier);
    public static native void storeSetActive(long handle, long elapsed, long boot);
    public static native long storeActiveAt(long handle);
    public static native long storeActiveBoot(long handle);
}
