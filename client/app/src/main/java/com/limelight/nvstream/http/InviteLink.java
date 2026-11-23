/**
 * Thin JNI delegate for {@code rubylight_protocol::invite_uri::InviteUri}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarJni;

import java.security.cert.X509Certificate;

public final class InviteLink {
    public static final String PREFIX = "rubylight://invite#";
    public static final String VIEW = "view";
    public static final String CONTROLLER = "controller";
    public static final String KEYBOARD_MOUSE = "controller-keyboard-mouse";

    public final String text;
    public final String name;
    public final long expires;
    public final String permission;
    public final String uniqueId;
    private final byte[] certSha256;
    private final String shownName;

    private InviteLink(Json.Obj o, String text) {
        this.text = text;
        this.name = o.getString("name");
        this.shownName = o.getString("shown_name");
        this.expires = o.getLong("expires");
        this.permission = o.getString("permission");
        this.uniqueId = o.getString("unique_id");
        this.certSha256 = PairingLink.hexBytes(o.getString("cert_sha256_hex"));
    }

    public static boolean looksLikeLink(String text) {
        if (text == null) return false;
        String raw = NightjarJni.uriCall(Json.obj().put("op", "invite_is_link").put("text", text).toString());
        return Json.parseObject(raw).optBoolean("ok", false);
    }

    public static InviteLink parse(String text) {
        if (text == null) return null;
        String raw = NightjarJni.uriCall(Json.obj().put("op", "invite_parse").put("text", text).toString());
        Json.Obj reply = Json.parseObject(raw);
        if (reply.isNull("ok")) return null;
        return new InviteLink(reply.getObject("ok"), text.trim());
    }

    public String shownName() { return shownName; }
    public boolean matches(X509Certificate certificate) { return certificate != null && PairingLink.matches(certSha256, certificate); }
    public boolean expired(long nowSeconds) { return nowSeconds >= expires; }
}
