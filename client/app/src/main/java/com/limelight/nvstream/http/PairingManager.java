/**
 * PIN and one-time-PIN pairing coordinator backed by {@code nightjar_core::http::pairing}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;

import org.xmlpull.v1.XmlPullParserException;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.security.SecureRandom;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.Locale;

public class PairingManager {
    private final NvHTTP http;
    private X509Certificate serverCert;

    public enum PairState {
        NOT_PAIRED,
        PAIRED,
        PIN_WRONG,
        FAILED,
        ALREADY_IN_PROGRESS,
        CERT_MISMATCH,
        SERVER_UNVERIFIED
    }

    public PairingManager(NvHTTP http, LimelightCryptoProvider cryptoProvider) {
        this.http = http;
    }

    public static String generatePinString() {
        SecureRandom r = new SecureRandom();
        return String.format((Locale) null, "%d%d%d%d",
                r.nextInt(10), r.nextInt(10), r.nextInt(10), r.nextInt(10));
    }

    public X509Certificate getPairedCert() {
        return serverCert;
    }

    public PairState pair(String serverInfo, String pin) throws IOException, XmlPullParserException {
        return pair(serverInfo, pin, null);
    }

    public PairState pair(String serverInfo, String pin, String passphrase) throws IOException, XmlPullParserException {
        return pair(serverInfo, pin, passphrase, null);
    }

    public PairState pair(String serverInfo, PairingLink link) throws IOException, XmlPullParserException {
        return pair(serverInfo, link.otp, link.passphrase, link.certSha256());
    }

    private PairState pair(String serverInfo, String pin, String passphrase, byte[] certSha256)
            throws IOException, XmlPullParserException {
        serverCert = null;
        String rawReply = NightjarHttpJni.pair(http.httpHandle(), serverInfo, pin, passphrase, certSha256);
        Json.Obj ok = NvHTTP.unwrapOkObj(rawReply, false);
        try {
            PairState state = PairState.valueOf(ok.getString("state"));
            if (state == PairState.PAIRED) {
                byte[] der = toByteArray(ok.getArray("server_certificate_der"));
                X509Certificate cert = (X509Certificate) CertificateFactory.getInstance("X.509")
                        .generateCertificate(new ByteArrayInputStream(der));
                serverCert = cert;
                http.setServerCert(cert);
            } else {
                http.setServerCert(null);
            }
            return state;
        } catch (Json.SyntaxException e) {
            throw new IOException("Malformed pairing reply", e);
        } catch (CertificateException e) {
            throw new IOException("Invalid pairing certificate", e);
        }
    }

    private static byte[] toByteArray(Json.Arr arr) {
        byte[] out = new byte[arr.length()];
        for (int i = 0; i < out.length; i++) {
            out[i] = (byte) arr.getInt(i);
        }
        return out;
    }
}
