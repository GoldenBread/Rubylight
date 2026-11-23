/**
 * Thin JNI delegate for {@code rubylight_protocol::pairing_uri::PairingUri}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarJni;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;
import java.util.Arrays;

public final class PairingLink {
    public static final String PREFIX = "rubylight://pair?";
    static final int MAX_LENGTH = 4096;

    public final String host;
    public final int port;
    public final String otp;
    public final String passphrase;
    public final String uniqueId;
    private final byte[] certSha256;
    public final String name;
    public final String text;
    private final String shownName;
    private final String shownAddress;

    private PairingLink(Json.Obj o, String text) {
        this.host = o.getString("host");
        this.port = o.getInt("port");
        this.otp = o.getString("otp");
        this.passphrase = o.getString("passphrase");
        this.uniqueId = o.getString("unique_id");
        this.certSha256 = hexBytes(o.getString("cert_sha256_hex"));
        this.name = o.getString("name");
        this.shownName = o.getString("shown_name");
        this.shownAddress = o.getString("shown_address");
        this.text = text;
    }

    public static boolean looksLikeLink(String text) {
        if (text == null) return false;
        String raw = NightjarJni.uriCall(Json.obj().put("op", "pairing_is_link").put("text", text).toString());
        return Json.parseObject(raw).optBoolean("ok", false);
    }

    public static PairingLink parse(String text) {
        if (text == null) return null;
        String raw = NightjarJni.uriCall(Json.obj().put("op", "pairing_parse").put("text", text).toString());
        Json.Obj reply = Json.parseObject(raw);
        if (reply.isNull("ok")) return null;
        return new PairingLink(reply.getObject("ok"), text.trim());
    }

    public String shownName() { return shownName; }
    public String shownAddress() { return shownAddress; }
    public boolean matches(X509Certificate certificate) { return matches(certSha256, certificate); }
    public byte[] certSha256() { return Arrays.copyOf(certSha256, certSha256.length); }

    static boolean matches(byte[] sha256, X509Certificate certificate) {
        if (sha256 == null || certificate == null) return false;
        try {
            byte[] actual = MessageDigest.getInstance("SHA-256").digest(certificate.getEncoded());
            return MessageDigest.isEqual(sha256, actual);
        } catch (CertificateEncodingException | NoSuchAlgorithmException e) {
            return false;
        }
    }

    static byte[] hexBytes(String hex) {
        byte[] out = new byte[hex.length() / 2];
        for (int i = 0; i < out.length; i++) {
            out[i] = (byte) Integer.parseInt(hex.substring(i * 2, i * 2 + 2), 16);
        }
        return out;
    }
}
