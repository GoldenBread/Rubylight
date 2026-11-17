package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;

/**
 * The simple settings layer's mapping on Android. The contract test holds {@link SimpleProfileModel} to
 * protocol/android/src/simple_profile_contract.json, which the Rust test checks the JNI entry answers
 * exactly; the rest reads the model's answers the way the settings rows do.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29)
public class SimpleProfileModelTest {
    private final List<JSONObject> asked = new ArrayList<>();
    private String answer;

    private static SimpleProfileModel.Capabilities hdrCapabilities() {
        return new SimpleProfileModel.Capabilities(1920, 1080, 120, true, true,
                true, true, true, true, true, true, true);
    }

    @Before
    public void setUp() {
        asked.clear();
        answer = "{\"ok\":{}}";
        SimpleProfileModel.use(request -> {
            try {
                asked.add(new JSONObject(request));
            } catch (Exception e) {
                throw new AssertionError(e);
            }
            return answer;
        });
    }

    @Test
    public void valuesAreReadFromTheModel() throws Exception {
        answer = "{\"ok\":{\"native_resolution\":true,\"fps\":null,\"pacing\":\"smoothest\",\"vrr\":false,"
                + "\"latency_switches\":true,\"drop_late_frames\":true,\"host_frame_times\":false,"
                + "\"real_vrr\":null,\"reduce_refresh\":false,\"content_rate\":false,"
                + "\"bitrate\":{\"mode\":\"factor\",\"milli\":1500},"
                + "\"codec\":\"av1\",\"hdr\":true,\"adaptive_bitrate\":false,\"adaptive_quality\":\"off\"}}";
        SimpleProfileModel.Values values = SimpleProfileModel.values("smoothest", "best", "auto", hdrCapabilities());
        assertEquals("smoothest", asked.get(0).getString("smoothness"));
        assertEquals("best", asked.get(0).getString("picture"));
        assertEquals("auto", asked.get(0).getString("hdr"));
        assertEquals(1500, values.bitrateMilli);
        assertEquals("av1", values.codec);
        assertEquals("smoothest", values.pacing);
        assertNull(values.fps);
        assertTrue(values.nativeResolution);
        assertTrue(values.hdr);
    }

    @Test
    public void aModelThatCannotAnswerChangesNothing() {
        SimpleProfileModel.use(request -> {
            throw new LinkageError("the library is not there in a JVM test");
        });
        assertNull(SimpleProfileModel.values("balanced", "auto", "auto", hdrCapabilities()));
        assertNull(SimpleProfileModel.recommended(hdrCapabilities()));
        assertNull(SimpleProfileModel.chosen(hdrCapabilities(), current()));
    }

    @Test
    public void recommendedAndChosenReadTheModelsAnswers() {
        answer = "{\"ok\":{\"smoothness\":\"balanced\",\"picture\":\"auto\",\"hdr\":\"auto\"}}";
        String[] recommended = SimpleProfileModel.recommended(hdrCapabilities());
        assertEquals("balanced", recommended[0]);
        assertEquals("auto", recommended[1]);
        assertEquals("auto", recommended[2]);

        answer = "{\"ok\":{\"choices\":null}}";
        assertNull(SimpleProfileModel.chosen(hdrCapabilities(), current()));
        answer = "{\"ok\":{\"choices\":{\"smoothness\":\"lowest-delay\",\"picture\":\"save-data\",\"hdr\":\"off\"}}}";
        String[] chosen = SimpleProfileModel.chosen(hdrCapabilities(), current());
        assertEquals("lowest-delay", chosen[0]);
        assertEquals("save-data", chosen[1]);
        assertEquals("off", chosen[2]);
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
            SimpleProfileModel.use(sent -> {
                try {
                    assertEquals(c.getString("name"), request.toString(),
                            sortedLike(request, new JSONObject(sent)).toString());
                    return c.getJSONObject("response").toString();
                } catch (Exception e) {
                    throw new AssertionError(e);
                }
            });
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
