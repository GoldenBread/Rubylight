package com.limelight.preferences;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorManager;

import com.limelight.binding.input.driver.AbstractController;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowSensor;
import org.robolectric.shadows.ShadowSensorManager;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class)
public class ControllerTestSensorsTest {
    private static final class Pad extends AbstractController {
        Pad() {
            super(1, null, 0x1234, 0x5678);
        }

        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short lowFreqMotor, short highFreqMotor) { }
        @Override public void rumbleTriggers(short leftTrigger, short rightTrigger) { }
    }

    /** A USB pad with motion that records the motion report rates it is asked for. */
    private static final class MotionPad extends AbstractController {
        final java.util.List<String> motion = new java.util.ArrayList<>();
        MotionPad() {
            super(2, null, 0x054c, 0x0ce6);
            capabilities = (short) (com.limelight.nvstream.jni.MoonBridge.LI_CCAP_ACCEL
                    | com.limelight.nvstream.jni.MoonBridge.LI_CCAP_GYRO);
        }

        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short lowFreqMotor, short highFreqMotor) { }
        @Override public void rumbleTriggers(short leftTrigger, short rightTrigger) { }
        @Override public void setMotionEventState(byte motionType, short reportRateHz) {
            motion.add(motionType + ":" + reportRateHz);
        }
    }

    @Test
    public void aRemovedPadLeavesNoMotionSensorListenerOfTheScreen() {
        Context app = RuntimeEnvironment.getApplication();
        app.getSharedPreferences(app.getPackageName() + "_preferences", Context.MODE_PRIVATE).edit()
                .putBoolean("checkbox_gamepad_motion_fallback", true).commit();
        SensorManager sensors = (SensorManager) app.getSystemService(Context.SENSOR_SERVICE);
        ShadowSensorManager shadow = Shadows.shadowOf(sensors);
        shadow.addSensor(ShadowSensor.newInstance(Sensor.TYPE_GYROSCOPE));
        shadow.addSensor(ShadowSensor.newInstance(Sensor.TYPE_ACCELEROMETER));

        ActivityController<ControllerTestActivity> controller =
                Robolectric.buildActivity(ControllerTestActivity.class).create().start().resume();
        ControllerTestActivity activity = controller.get();
        Pad pad = new Pad();
        activity.deviceAdded(pad);
        assertTrue("the device's sensors are in use while the pad is shown", shadow.hasListener(activity));

        activity.deviceRemoved(pad);
        assertFalse("nothing of the screen keeps listening once the pad is gone", shadow.hasListener(activity));
    }

    @Test
    public void aUsbPadsMotionReportsAreAskedForAgainWhenTheScreenIsReopened() {
        ActivityController<ControllerTestActivity> controller =
                Robolectric.buildActivity(ControllerTestActivity.class).create().start().resume();
        ControllerTestActivity activity = controller.get();
        MotionPad pad = new MotionPad();
        activity.deviceAdded(pad);
        int accel = com.limelight.nvstream.jni.MoonBridge.LI_MOTION_TYPE_ACCEL;
        int gyro = com.limelight.nvstream.jni.MoonBridge.LI_MOTION_TYPE_GYRO;
        pad.motion.clear();

        controller.pause();
        assertTrue("pausing turns the reports off", pad.motion.contains(accel + ":0") && pad.motion.contains(gyro + ":0"));
        pad.motion.clear();
        controller.resume();
        assertTrue("resuming asks for them again",
                pad.motion.contains(accel + ":60") && pad.motion.contains(gyro + ":60"));
    }
}
