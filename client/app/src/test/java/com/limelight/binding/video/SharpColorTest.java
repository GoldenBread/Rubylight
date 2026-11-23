package com.limelight.binding.video;

import android.app.Application;

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

import static org.junit.Assert.*;

/**
 * Sharper colored text over JNI. The contract test holds {@link SharpColor} to
 * protocol/client/src/sharp_color_contract.json, running directly against {@link NightjarJni#sharpColorCall}:
 * each answer read by Java matches the shared Rust policy end-to-end.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class SharpColorTest {
    private static JSONArray contract() throws Exception {
        for (String path : new String[] {"../protocol/client/src/sharp_color_contract.json", "protocol/client/src/sharp_color_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("sharp_color_contract.json not found from " + new File(".").getAbsolutePath());
    }

    private static SharpColor.Request request(JSONObject json) throws Exception {
        SharpColor.Request request = new SharpColor.Request();
        request.auto = json.getBoolean("auto");
        request.hdr = json.getBoolean("hdr");
        request.sdr10 = json.getBoolean("sdr10");
        request.pyroWave = json.getBoolean("pyrowave");
        request.packedEngine = json.getBoolean("packed_engine");
        request.base = json.getInt("base");
        request.decodable = json.getInt("decodable");
        request.decoderReason = json.getString("decoder_reason");
        request.hostScm = json.getInt("host_scm");
        request.hostPacked = json.isNull("host_packed") ? -1 : json.getInt("host_packed");
        request.packedRefusedBefore = json.getBoolean("packed_refused_before");
        return request;
    }

    @Test
    public void theBindingSpeaksTheSharedContract() throws Exception {
        JSONArray cases = contract();
        int decided = 0;
        for (int i = 0; i < cases.length(); i++) {
            JSONObject c = cases.getJSONObject(i);
            JSONObject ok = c.getJSONObject("response").getJSONObject("ok");
            if (ok.has("need_device_modes")) continue;
            JSONObject sent = c.getJSONObject("request");
            int[] probes = {0};
            boolean verified = !sent.isNull("device_modes");
            SharpColor.Pick pick = SharpColor.decide(request(sent), () -> {
                probes[0]++;
                try {
                    return verified ? sent.getInt("device_modes") : 0;
                } catch (org.json.JSONException e) {
                    throw new AssertionError(e);
                }
            });
            String name = c.getString("name");
            // The device's modes cost a decode: asked only when the case carries them.
            assertEquals(name, verified ? 1 : 0, probes[0]);
            switch (ok.getString("pick")) {
                case "native":
                    assertEquals(name, SharpColor.Pick.Kind.NATIVE, pick.kind);
                    assertEquals(name, ok.getInt("formats"), pick.formats);
                    break;
                case "packed": {
                    assertEquals(name, SharpColor.Pick.Kind.PACKED, pick.kind);
                    assertEquals(name, ok.getInt("modes"), pick.modes);
                    String nativeWhy = ok.optString("native_why", "");
                    String nativeDetail = ok.optString("native_detail", "");
                    assertEquals(name, nativeWhy.isEmpty() ? null : nativeWhy, pick.nativeWhy);
                    assertEquals(name, nativeDetail.isEmpty() ? null : nativeDetail, pick.nativeDetail);
                    assertTrue(name + ": toString should explain why native was skipped: " + pick,
                            pick.toString().contains("native 4:4:4 skipped: " + nativeWhy));
                    break;
                }
                default:
                    assertEquals(name, SharpColor.Pick.Kind.WITHOUT, pick.kind);
                    assertEquals(name, ok.getString("why"), pick.why);
                    String detail = ok.optString("detail", "");
                    assertEquals(name, detail.isEmpty() ? null : detail, pick.detail);
                    assertEquals(name, ok.getBoolean("tell"), pick.worthSaying());
            }
            decided++;
        }
        assertTrue(decided >= 6);
    }

    @Test
    public void onlyAReasonThePlayerCanActOnIsSaid() throws Exception {
        for (String why : new String[] {"off", "pyrowave", "hdr", SharpColor.Pick.UNANSWERED}) {
            assertFalse(why, SharpColor.read(new JSONObject().put("pick", "without").put("why", why)).worthSaying());
        }
        for (String why : new String[] {"decoder", "host", "device", "no_common_mode"}) {
            assertTrue(why, SharpColor.read(new JSONObject().put("pick", "without").put("why", why)).worthSaying());
        }
        assertTrue(SharpColor.read(new JSONObject().put("pick", "without").put("why", "no_common_mode")).fromHost());
        assertFalse(SharpColor.read(new JSONObject().put("pick", "without").put("why", "device")).fromHost());
        assertFalse(SharpColor.read(new JSONObject().put("pick", "packed").put("modes", 1)).worthSaying());
    }

    @Test
    public void aModelThatCannotAnswerStartsWithoutEitherAndSilently() {
        NightjarJni.forceUnavailableForTest(new UnsatisfiedLinkError("no library"));
        try {
            SharpColor.Pick pick = SharpColor.decide(new SharpColor.Request(), () -> { throw new AssertionError("no probe"); });
            assertEquals(SharpColor.Pick.Kind.WITHOUT, pick.kind);
            assertFalse(pick.worthSaying());
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
        SharpColor.Pick invalidJson = SharpColor.parseAnswer("{\"error\":\"invalid\",\"detail\":\"x\"}");
        assertEquals(SharpColor.Pick.Kind.WITHOUT, invalidJson.kind);
        assertFalse(invalidJson.worthSaying());
        SharpColor.Pick invalidCall = SharpColor.parseAnswer(NightjarJni.sharpColorCall("not json"));
        assertEquals(SharpColor.Pick.Kind.WITHOUT, invalidCall.kind);
        assertFalse(invalidCall.worthSaying());
    }
}
