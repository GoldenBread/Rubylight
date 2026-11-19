package com.limelight.mic;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.net.wifi.WifiManager;
import android.os.PowerManager;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;

import java.lang.reflect.Field;
import java.lang.reflect.Method;

/** Microphone only's CPU wake lock does not depend on the Wi-Fi locks, which some devices refuse. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = MicOnlyLocksTest.RefusingWifiLock.class)
public class MicOnlyLocksTest {
    /** A Wi-Fi lock whose acquire is refused, and whose release throws unless it is held, as on a device. */
    @Implements(WifiManager.WifiLock.class)
    public static class RefusingWifiLock extends org.robolectric.shadows.ShadowWifiManager.ShadowWifiLock {
        @Implementation
        public void acquire() {
            throw new SecurityException("refused");
        }

        @Implementation
        public void release() {
            throw new RuntimeException("not held");
        }
    }

    @Test
    public void aRefusedWifiLockLeavesTheWakeLockHeldAndReleasingWorks() throws Exception {
        MicOnlyService service = Robolectric.buildService(MicOnlyService.class).get();
        Method acquire = MicOnlyService.class.getDeclaredMethod("acquireLocks");
        acquire.setAccessible(true);
        acquire.invoke(service);
        PowerManager.WakeLock wake = (PowerManager.WakeLock) field("wakeLock").get(service);
        assertNotNull(wake);
        assertTrue(wake.isHeld());

        Method release = MicOnlyService.class.getDeclaredMethod("releaseLocks");
        release.setAccessible(true);
        release.invoke(service);
        assertFalse(wake.isHeld());
        assertNull(field("wifiLock").get(service));
    }

    private static Field field(String name) throws Exception {
        Field field = MicOnlyService.class.getDeclaredField(name);
        field.setAccessible(true);
        return field;
    }
}
