/**
 * Thin JNI delegate for {@code rubylight_protocol::invite_uri::InviteUri}.
 */
package com.limelight.nvstream.http;

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

    private InviteLink(String[] f, String text) {
        this.text = text;
        this.name = f[0];
        this.shownName = f[1];
        this.expires = Long.parseLong(f[2]);
        this.permission = f[3];
        this.uniqueId = f[4];
        this.certSha256 = PairingLink.hexBytes(f[5]);
    }

    public static boolean looksLikeLink(String text) {
        return text != null && "1".equals(NightjarJni.uriFields("invite_is_link", text));
    }

    public static InviteLink parse(String text) {
        if (text == null) return null;
        String raw = NightjarJni.uriFields("invite_parse", text);
        if (raw == null) return null;
        String[] f = raw.split("\0", -1);
        return f.length == 6 ? new InviteLink(f, text.trim()) : null;
    }

    public String shownName() { return shownName; }
    public boolean matches(X509Certificate certificate) { return certificate != null && PairingLink.matches(certSha256, certificate); }
    public boolean expired(long nowSeconds) { return nowSeconds >= expires; }
}
