package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.jni.NightjarJni;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

/**
 * The simple settings layer's mapping on Android, backed directly by {@link NightjarJni#simpleProfileCall}.
 * The contract test holds {@link SimpleProfileModel} to protocol/android/src/simple_profile_contract.json end-to-end.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29)
public class SimpleProfileModelTest {
    private static SimpleProfileModel.Capabilities hdrCapabilities() {
        return new SimpleProfileModel.Capabilities(1920, 1080, 120, true, true,
                true, true, true, true, true, true, true);
    }

    @Test
    public void valuesAreReadFromTheModel() {
        SimpleProfileModel.Values values = SimpleProfileModel.values("smoothest", "best", "auto", hdrCapabilities());
        assertEquals(1500, values.bitrateMilli);
        assertEquals("auto", values.codec);
        assertEquals("smoothest", values.pacing);
        assertNull(values.fps);
        assertTrue(values.nativeResolution);
        assertTrue(values.hdr);
    }

    @Test
    public void aModelThatCannotAnswerChangesNothing() {
        NightjarJni.forceUnavailableForTest(new LinkageError("the library is not there in a JVM test"));
        try {
            assertNull(SimpleProfileModel.values("balanced", "auto", "auto", hdrCapabilities()));
            assertNull(SimpleProfileModel.recommended(hdrCapabilities()));
            assertNull(SimpleProfileModel.chosen(hdrCapabilities(), current()));
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
        assertNull(SimpleProfileModel.parseAnswer("{\"error\":\"invalid\"}"));
    }

    @Test
    public void recommendedAndChosenReadTheModelsAnswers() {
        String[] recommended = SimpleProfileModel.recommended(hdrCapabilities());
        assertEquals("auto", recommended[0]);
        assertEquals("auto", recommended[1]);
        assertEquals("auto", recommended[2]);

        SimpleProfileModel.Current vrrAuto = new SimpleProfileModel.Current(true, null, "balanced", true, true,
                true, false, true, false, false, null, "auto", true, false, "off",
                "balanced", "auto", "auto");
        String[] chosen = SimpleProfileModel.chosen(hdrCapabilities(), vrrAuto);
        assertEquals("balanced", chosen[0]);
        assertEquals("auto", chosen[1]);
        assertEquals("auto", chosen[2]);

        SimpleProfileModel.Current custom = new SimpleProfileModel.Current(true, 60, "balanced", true, true,
                true, false, true, false, false, null, "auto", true, false, "off",
                "balanced", "auto", "auto");
        assertNull(SimpleProfileModel.chosen(hdrCapabilities(), custom));
    }

    private static SimpleProfileModel.Current current() {
        return new SimpleProfileModel.Current(true, null, "balanced", true, true,
                true, false, true, false, false, null, "auto", true, false, "off",
                "balanced", "auto", "auto");
    }

    private static JSONArray contract() throws Exception {
        for (String path : new String[] {"../protocol/android/src/simple_profile_contract.json",
                "protocol/android/src/simple_profile_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("simple_profile_contract.json not found from " + new File(".").getAbsolutePath());
    }

    /** Each request the model sends is the shared contract's, and it reads the answer there as meant. */
    @Test
    public void theModelSpeaksTheSharedContract() throws Exception {
        JSONArray cases = contract();
        for (int i = 0; i < cases.length(); i++) {
            JSONObject c = cases.getJSONObject(i);
            JSONObject request = c.getJSONObject("request");
            JSONObject ok = c.getJSONObject("response").getJSONObject("ok");
            switch (request.getString("op")) {
                case "values": {
                    SimpleProfileModel.Values values = SimpleProfileModel.values(
                            request.optString("smoothness", "balanced"), request.optString("picture", "auto"),
                            request.optString("hdr", "auto"), capabilitiesOf(request));
                    assertEquals(c.getString("name"), ok.getString("pacing"), values.pacing);
                    assertEquals(c.getString("name"), ok.getBoolean("vrr"), values.vrr);
                    assertEquals(c.getString("name"), ok.getBoolean("hdr"), values.hdr);
                    assertEquals(c.getString("name"), ok.getBoolean("adaptive_bitrate"), values.adaptiveBitrate);
                    assertEquals(c.getString("name"), ok.getString("adaptive_quality"), values.adaptiveQuality);
                    JSONObject bitrate = ok.getJSONObject("bitrate");
                    assertEquals(c.getString("name"), "factor".equals(bitrate.getString("mode"))
                            ? bitrate.getInt("milli") : 1000, values.bitrateMilli);
                    break;
                }
                case "recommended": {
                    String[] choices = SimpleProfileModel.recommended(capabilitiesOf(request));
                    assertEquals(c.getString("name"), ok.getString("smoothness"), choices[0]);
                    assertEquals(c.getString("name"), ok.getString("picture"), choices[1]);
                    assertEquals(c.getString("name"), ok.getString("hdr"), choices[2]);
                    break;
                }
                case "chosen": {
                    JSONObject current = request.getJSONObject("current");
                    String[] choices = SimpleProfileModel.chosen(capabilitiesOf(request),
                            new SimpleProfileModel.Current(current.getBoolean("native_resolution"),
                                    current.isNull("fps") ? null : current.getInt("fps"), current.getString("pacing"),
                                    current.getBoolean("vrr"), current.getBoolean("latency_switches"),
                                    current.getBoolean("drop_late_frames"), current.getBoolean("host_frame_times"),
                                    current.getBoolean("real_vrr"), current.getBoolean("reduce_refresh"),
                                    current.getBoolean("content_rate"),
                                    current.isNull("bitrate_milli") ? null : current.getInt("bitrate_milli"),
                                    current.getString("codec"), current.getBoolean("hdr"),
                                    current.getBoolean("adaptive_bitrate"), current.getString("adaptive_quality"),
                                    current.isNull("stored_smoothness") ? null : current.getString("stored_smoothness"),
                                    current.isNull("stored_picture") ? null : current.getString("stored_picture"),
                                    current.isNull("stored_hdr") ? null : current.getString("stored_hdr")));
                    if (ok.isNull("choices")) {
                        assertNull(c.getString("name"), choices);
                    } else {
                        JSONObject expected = ok.getJSONObject("choices");
                        assertEquals(c.getString("name"), expected.getString("smoothness"), choices[0]);
                        assertEquals(c.getString("name"), expected.getString("picture"), choices[1]);
                        assertEquals(c.getString("name"), expected.getString("hdr"), choices[2]);
                    }
                    break;
                }
                default:
                    throw new AssertionError("unexpected op " + request.getString("op"));
            }
        }
        assertTrue(cases.length() > 0);
    }

    private static SimpleProfileModel.Capabilities capabilitiesOf(JSONObject request) throws Exception {
        JSONObject display = request.getJSONObject("display");
        JSONObject decoder = request.getJSONObject("decoder");
        JSONObject host = request.getJSONObject("host");
        return new SimpleProfileModel.Capabilities(display.getInt("width"), display.getInt("height"),
                display.getInt("refresh_hz"), display.getBoolean("hdr"), display.getBoolean("vrr"),
                decoder.getBoolean("h264"), decoder.getBoolean("hevc"), decoder.getBoolean("av1"),
                decoder.getBoolean("hdr"),
                host.isNull("hdr") ? null : host.getBoolean("hdr"),
                host.isNull("hevc") ? null : host.getBoolean("hevc"),
                host.isNull("av1") ? null : host.getBoolean("av1"));
    }

    /** The JSON of {@code sent} in the key order of {@code like}, failing when the keys differ. */
    private static JSONObject sortedLike(JSONObject like, JSONObject sent) throws Exception {
        assertEquals(like.length(), sent.length());
        JSONObject sorted = new JSONObject();
        for (java.util.Iterator<String> keys = like.keys(); keys.hasNext();) {
            String key = keys.next();
            sorted.put(key, sent.get(key));
        }
        return sorted;
    }
}
