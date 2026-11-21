package com.limelight.binding.video;

import org.junit.Test;

import java.lang.reflect.Field;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.Assert.*;

/**
 * A native submit holds nativeLock, not the object monitor: the statistics and health getters must not wait for it,
 * while teardown (cleanup) must, so it cannot destroy the renderer under a submit in flight.
 */
public class PyroWaveRendererLockingTest {
    private static Object nativeLock(PyroWaveDecoderRenderer renderer) throws Exception {
        Field field = PyroWaveDecoderRenderer.class.getDeclaredField("nativeLock");
        field.setAccessible(true);
        return field.get(renderer);
    }

    /** Holds the lock the way a submit in native code does, until released. */
    private static Thread holdNativeLock(Object lock, CountDownLatch held, CountDownLatch release) {
        Thread thread = new Thread(() -> {
            synchronized (lock) {
                held.countDown();
                try {
                    release.await();
                } catch (InterruptedException ignored) {
                }
            }
        });
        thread.start();
        return thread;
    }

    @Test
    public void gettersDoNotWaitForASubmitInFlightButCleanupDoes() throws Exception {
        PyroWaveDecoderRenderer renderer = new PyroWaveDecoderRenderer();
        CountDownLatch held = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        Thread submit = holdNativeLock(nativeLock(renderer), held, release);
        assertTrue(held.await(5, TimeUnit.SECONDS));

        AtomicBoolean gettersDone = new AtomicBoolean();
        Thread getters = new Thread(() -> {
            renderer.getDriver();
            renderer.getWaits();
            renderer.getUpscalingReason();
            renderer.getPresentMode();
            renderer.getLastGpuDecodeUs();
            renderer.getEarlyRecords();
            renderer.getLastSubmissionDurationNs();
            renderer.hasMaxClocks();
            gettersDone.set(true);
        });
        getters.start();
        getters.join(5000);
        assertTrue("the getters waited for the submit", gettersDone.get());

        AtomicBoolean cleanedUp = new AtomicBoolean();
        Thread cleanup = new Thread(() -> {
            renderer.cleanup();
            cleanedUp.set(true);
        });
        cleanup.start();
        cleanup.join(300);
        assertFalse("cleanup must wait for the submit in flight", cleanedUp.get());

        release.countDown();
        cleanup.join(5000);
        submit.join(5000);
        assertTrue(cleanedUp.get());
    }
}
