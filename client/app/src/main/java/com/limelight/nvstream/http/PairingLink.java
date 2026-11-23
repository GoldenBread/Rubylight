/**
 * Thin JNI delegate for {@code rubylight_protocol::pairing_uri::PairingUri}.
 */
package com.limelight.nvstream.http;

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

    private PairingLink(String[] f, String text) {
        this.host = f[0];
        this.port = Integer.parseInt(f[1]);
        this.otp = f[2];
        this.passphrase = f[3];
        this.uniqueId = f[4];
        this.certSha256 = hexBytes(f[5]);
        this.name = f[6];
        this.shownName = f[7];
        this.shownAddress = f[8];
        this.text = text;
    }

    public static boolean looksLikeLink(String text) {
        return text != null && "1".equals(NightjarJni.uriFields("pairing_is_link", text));
    }

    public static PairingLink parse(String text) {
        if (text == null) return null;
        String raw = NightjarJni.uriFields("pairing_parse", text);
        if (raw == null) return null;
        String[] f = raw.split("\0", -1);
        return f.length == 9 ? new PairingLink(f, text.trim()) : null;
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
