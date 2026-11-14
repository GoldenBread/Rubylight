package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/**
 * Frame pacing's merged row reads and writes the keys it always had (frame_pacing, checkbox_drop_late_frames,
 * checkbox_host_frame_times), and the lower refresh row its two switches: no stored value changes meaning, in the
 * main settings or in a PC, game or profile layer saved before.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class)
public class FramePacingChoiceTest {
    private static final String[] PACINGS = {"latency", "balanced", "cap-fps", "smoothness", "something-old"};

    private SharedPreferences prefs;

    @Before
    public void clear() {
        prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
    }

    /** What a stream did with these stored values before the merge: the PC's frame times replace the pacing. */
    private static String streamed(String pacing, boolean dropLate, boolean hostTimes) {
        if (hostTimes) return "host";
        int value = PreferenceConfiguration.framePacingValue(pacing);
        if (value == PreferenceConfiguration.FRAME_PACING_BALANCED) return dropLate ? "balanced" : "balanced-every";
        if (value == PreferenceConfiguration.FRAME_PACING_CAP_FPS) return "cap-fps";
        if (value == PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS) return "smoothness";
        return "latency";
    }

    @Test
    public void everyStoredCombinationReadsAsTheChoiceItStreamed() {
        for (String pacing : PACINGS) {
            for (boolean dropLate : new boolean[] {true, false}) {
                for (boolean hostTimes : new boolean[] {true, false}) {
                    String what = pacing + " newest=" + dropLate + " host=" + hostTimes;
                    prefs.edit().clear().putString("frame_pacing", pacing).putBoolean("checkbox_drop_late_frames", dropLate)
                            .putBoolean("checkbox_host_frame_times", hostTimes).commit();
                    String choice = FramePacingChoice.read(prefs);
                    assertEquals(what, streamed(pacing, dropLate, hostTimes), choice);

                    // Saving the choice it reads changes nothing a stream uses.
                    SharedPreferences.Editor editor = prefs.edit();
                    FramePacingChoice.save(editor, choice);
                    editor.commit();
                    assertEquals(what, choice, FramePacingChoice.read(prefs));
                    assertEquals(what, hostTimes, prefs.getBoolean("checkbox_host_frame_times", false));
                    if (hostTimes) {
                        // The pacing under the PC's frame times is kept for when it is chosen again.
                        assertEquals(what, pacing, prefs.getString("frame_pacing", null));
                    }
                    if (!"balanced".equals(pacing)) {
                        // Keep only the newest frame is Balanced's alone: the other choices leave it.
                        assertEquals(what, dropLate, prefs.getBoolean("checkbox_drop_late_frames", true));
                    }
                }
            }
        }
    }

    @Test
    public void eachChoiceWritesTheKeysOfItsPacing() {
        for (String choice : FramePacingChoice.VALUES) {
            prefs.edit().clear().putString("frame_pacing", "smoothness").putBoolean("checkbox_drop_late_frames", true)
                    .putBoolean("checkbox_host_frame_times", true).commit();
            SharedPreferences.Editor editor = prefs.edit();
            FramePacingChoice.save(editor, choice);
            editor.commit();
            assertEquals(choice, choice, FramePacingChoice.read(prefs));
        }
        // An unknown value writes nothing.
        SharedPreferences.Editor editor = prefs.edit();
        FramePacingChoice.save(editor, "nonsense");
        editor.commit();
        assertEquals("host", FramePacingChoice.read(prefs));
    }

    @Test
    public void absentKeysReadAsLowestLatency() {
        assertEquals(PreferenceConfiguration.DEFAULT_FRAME_PACING, FramePacingChoice.read(prefs));
    }

    @Test
    public void streamValuesReadAsTheirChoiceAndVrrHidesThePcsFrameTimes() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
        config.dropLateFrames = false;
        assertEquals("balanced-every", FramePacingChoice.of(config));
        config.hostFrameTimes = true;
        assertEquals("host", FramePacingChoice.of(config));
        config.vrr = true;
        assertEquals("Real VRR is Variable refresh rate's", "balanced-every", FramePacingChoice.of(config));
    }

    /**
     * A layer saved before the merge holds only its pacing: the stream takes the PC's frame times and Keep only the
     * newest frame from the main settings, as it did, so the main PC's-frame-times choice still beats the layer.
     */
    @Test
    public void anOldLayerKeepsWhatItStreamedWith() {
        for (String layerPacing : new String[] {"latency", "balanced", "cap-fps", "smoothness"}) {
            HostStreamProfile layer = HostStreamProfile.deserialize("v2;pacing=" + layerPacing);
            assertNull(layer.hostFrameTimes);
            assertNull(layer.dropLateFrames);
            for (boolean dropLate : new boolean[] {true, false}) {
                for (boolean hostTimes : new boolean[] {true, false}) {
                    PreferenceConfiguration config = new PreferenceConfiguration();
                    config.dropLateFrames = dropLate;
                    config.gridHostFrameTimes = hostTimes;
                    layer.applyTo(config);
                    config.resolveHostFrameTimes();
                    assertEquals(dropLate, config.dropLateFrames);
                    assertEquals(hostTimes, config.hostFrameTimes);
                    assertEquals(layerPacing, streamed(layerPacing, dropLate, hostTimes), FramePacingChoice.of(config));
                }
            }
        }
    }

    @Test
    public void aLayersChoiceCarriesItsThreeValues() {
        for (String choice : FramePacingChoice.VALUES) {
            HostStreamProfile own = new HostStreamProfile();
            FramePacingChoice.save(own, choice);
            HostStreamProfile read = HostStreamProfile.deserialize(own.serialize());
            assertTrue(choice, read.has(HostStreamProfile.Field.PACING));
            assertEquals(choice + ": one value", 1, read.count());
            // Over main settings that hold the opposite of everything.
            PreferenceConfiguration config = new PreferenceConfiguration();
            config.framePacing = PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS;
            config.gridHostFrameTimes = !"host".equals(choice);
            config.dropLateFrames = "balanced-every".equals(choice);
            read.applyTo(config);
            config.resolveHostFrameTimes();
            if (read.pacing != null) config.framePacing = PreferenceConfiguration.framePacingValue(read.pacing);
            assertEquals(choice, choice, FramePacingChoice.of(config));

            HostStreamProfile taken = new HostStreamProfile();
            taken.take(HostStreamProfile.Field.PACING, read);
            assertEquals(choice, own.serialize(), taken.serialize());
        }
        HostStreamProfile own = new HostStreamProfile();
        FramePacingChoice.save(own, "host");
        FramePacingChoice.save(own, null);
        assertTrue("the main settings' again", own.isEmpty());
    }

    @Test
    public void companionValuesNeverStandAlone() {
        // Old builds wrote neither; a value without its pacing is not a value (as Real VRR without VRR).
        assertTrue(HostStreamProfile.deserialize("v2;host_times=true").isEmpty());
        assertTrue(HostStreamProfile.deserialize("v2;newest=false").isEmpty());
        assertNull(HostStreamProfile.deserialize("v2;pacing=latency;newest=false").dropLateFrames);
        assertEquals(Boolean.FALSE, HostStreamProfile.deserialize("v2;pacing=balanced;newest=false").dropLateFrames);
        assertEquals(Boolean.TRUE, HostStreamProfile.deserialize("v2;pacing=latency;host_times=true").hostFrameTimes);
    }

    @Test
    public void lowerRefreshReadsBothSwitchesAsOneChoice() {
        boolean[] both = {false, true};
        for (boolean reduce : both) {
            for (boolean content : both) {
                prefs.edit().clear().putBoolean("checkbox_reduce_refresh_rate", reduce)
                        .putBoolean("checkbox_content_rate_frame_rate", content).commit();
                int choice = LowerRefreshChoice.read(prefs);
                assertEquals(!reduce ? LowerRefreshChoice.OFF : content ? LowerRefreshChoice.MATCHED : LowerRefreshChoice.ON, choice);
                SharedPreferences.Editor editor = prefs.edit();
                LowerRefreshChoice.save(editor, choice);
                editor.commit();
                // Off keeps the matched choice for when the lower refresh comes back, as the hidden switch did.
                assertEquals(reduce, prefs.getBoolean("checkbox_reduce_refresh_rate", false));
                assertEquals(content, prefs.getBoolean("checkbox_content_rate_frame_rate", false));
            }
        }
        for (int i = 0; i < LowerRefreshChoice.VALUES.length; i++) {
            assertEquals(i, LowerRefreshChoice.ofValue(LowerRefreshChoice.VALUES[i]));
            SharedPreferences.Editor editor = prefs.edit();
            LowerRefreshChoice.save(editor, i);
            editor.commit();
            assertEquals(i, LowerRefreshChoice.read(prefs));
        }
        assertEquals(LowerRefreshChoice.OFF, LowerRefreshChoice.ofValue("unknown"));
        prefs.edit().clear().commit();
        assertEquals(LowerRefreshChoice.OFF, LowerRefreshChoice.read(prefs));
    }
}
