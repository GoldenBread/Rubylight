package com.limelight.ui;

import org.robolectric.pluginapi.TestEnvironmentLifecyclePlugin;

import java.lang.reflect.Field;

/**
 * Starts every Robolectric test with a fresh androidx spring animation handler (listed in META-INF/services).
 *
 * <p>androidx.dynamicanimation keeps one AnimationHandler per thread, and Robolectric reuses the main thread while it
 * drops the Choreographer's pending frames between tests. A spring still running when a test ended lost its frame, and
 * as the handler posts a frame only while its list is empty, no spring ran again in that JVM: the list kept every
 * spring started since, each holding its view and with it the whole screen and its themes, until the heap ran out.
 */
public final class FreshSpringHandler implements TestEnvironmentLifecyclePlugin {
    @Override public void onSetupApplicationState() {
        try {
            Class<?> handler = Class.forName("androidx.dynamicanimation.animation.AnimationHandler", false,
                    Thread.currentThread().getContextClassLoader());
            Field perThread = handler.getDeclaredField("sAnimatorHandler");
            perThread.setAccessible(true);
            ((ThreadLocal<?>) perThread.get(null)).remove();
        } catch (ReflectiveOperationException e) {
            throw new AssertionError("androidx.dynamicanimation's AnimationHandler changed: update this plugin", e);
        }
    }
}
