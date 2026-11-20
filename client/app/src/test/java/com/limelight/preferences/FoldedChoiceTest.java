package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/** Vibration, Trackpad acceleration and Glide after a flick: a stored switch folds into its list once. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class FoldedChoiceTest {
    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    @Test
    public void turnedOffVibrationBecomesOffWhateverTheFeelWas() {
        prefs().edit().putBoolean("checkbox_disable_rumble", true).putString("list_rumble_feel", "strong").commit();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
        assertTrue(config.disableRumble);
        assertEquals("off", prefs().getString("list_rumble_feel", null));
        assertFalse(prefs().contains("checkbox_disable_rumble"));
        assertEquals("standard", config.rumbleFeel);
    }

    @Test
    public void vibrationThatWasOnKeepsItsFeel() {
        prefs().edit().putBoolean("checkbox_disable_rumble", false).putString("list_rumble_feel", "subtle").commit();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
        assertFalse(config.disableRumble);
        assertEquals("subtle", config.rumbleFeel);
        assertEquals("subtle", prefs().getString("list_rumble_feel", null));
        assertFalse(prefs().contains("checkbox_disable_rumble"));
    }

    @Test
    public void aNewInstallVibratesStandardAcceleratesMediumAndDoesNotGlide() {
        PreferenceManager.setDefaultValues(app(), R.xml.preferences, true);
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
        assertFalse(config.disableRumble);
        assertEquals("standard", config.rumbleFeel);
        assertTrue(config.trackpadAcceleration);
        assertEquals("medium", config.trackpadAccelerationCurve);
        assertFalse(config.trackpadMomentum);
    }

    @Test
    public void accelerationOffBecomesOffAndOnKeepsItsStrength() {
        prefs().edit().putBoolean("checkbox_trackpad_acceleration_v2", false)
                .putString("list_trackpad_acceleration_curve", "gentle").commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertEquals("off", prefs().getString("list_trackpad_acceleration_curve", null));

        prefs().edit().clear().putBoolean("checkbox_trackpad_acceleration_v2", true)
                .putString("list_trackpad_acceleration_curve", "strong").commit();
        PreferenceConfiguration on = PreferenceConfiguration.readPreferences(app());
        assertTrue(on.trackpadAcceleration);
        assertEquals("strong", on.trackpadAccelerationCurve);
        assertFalse(prefs().contains("checkbox_trackpad_acceleration_v2"));
    }

    @Test
    public void theFirstAccelerationSwitchIsForgottenAndCountsAsOn() {
        prefs().edit().putBoolean("checkbox_trackpad_acceleration", false).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).trackpadAcceleration);
        assertFalse(prefs().contains("checkbox_trackpad_acceleration"));
    }

    @Test
    public void glideOnKeepsItsDistanceAndOffBecomesOff() {
        prefs().edit().putBoolean("checkbox_trackpad_momentum", true).putString("list_trackpad_momentum_glide", "long").commit();
        PreferenceConfiguration on = PreferenceConfiguration.readPreferences(app());
        assertTrue(on.trackpadMomentum);
        assertEquals("long", on.trackpadMomentumGlide);

        prefs().edit().clear().putBoolean("checkbox_trackpad_momentum", false).putString("list_trackpad_momentum_glide", "long").commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).trackpadMomentum);
        assertEquals("off", prefs().getString("list_trackpad_momentum_glide", null));
    }

    @Test
    public void aValueChosenAfterTheMigrationIsKept() {
        prefs().edit().putBoolean("checkbox_disable_rumble", true).commit();
        PreferenceConfiguration.readPreferences(app());
        prefs().edit().putString("list_rumble_feel", "strong").commit();
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
        assertFalse(config.disableRumble);
        assertEquals("strong", config.rumbleFeel);
        assertFalse(FoldedChoice.migrate(prefs()));
    }

    @Test
    public void aStoredTemporalUpscalingBecomesSgsr() {
        prefs().edit().putString("upscaling_mode", "sgsr2").commit();
        assertEquals(com.limelight.binding.video.UpscalingPolicy.Mode.SGSR1,
                PreferenceConfiguration.readPreferences(app()).upscalingMode);
        assertEquals("sgsr1", prefs().getString("upscaling_mode", null));
        prefs().edit().putString("upscaling_mode", "fsr1").commit();
        PreferenceConfiguration.readPreferences(app());
        assertEquals("fsr1", prefs().getString("upscaling_mode", null));
    }
}
