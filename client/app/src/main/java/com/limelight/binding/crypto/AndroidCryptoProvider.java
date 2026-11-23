/**
 * Android client pairing certificate & key provider backed by Rust {@code IdentityStore}
 * ({@code identity.json}) via {@link NightjarHostsJni} and standard JCA certificate/key decoding
 * (zero BouncyCastle runtime dependency).
 */
package com.limelight.binding.crypto;

import android.content.Context;
import android.util.Base64;

import com.limelight.LimeLog;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHostsJni;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.KeyFactory;
import java.security.NoSuchAlgorithmException;
import java.security.PrivateKey;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.security.spec.InvalidKeySpecException;
import java.security.spec.PKCS8EncodedKeySpec;

public class AndroidCryptoProvider implements LimelightCryptoProvider {
    private static final Object globalCryptoLock = new Object();

    private final File filesDir;
    private X509Certificate cert;
    private PrivateKey key;
    private byte[] pemCertBytes;

    public AndroidCryptoProvider(Context c) {
        this(c.getApplicationContext().getFilesDir());
    }

    AndroidCryptoProvider(File filesDir) {
        this.filesDir = filesDir;
    }

    private static byte[] readFileIfPresent(File f) {
        if (f == null || !f.isFile()) {
            return null;
        }
        try (FileInputStream in = new FileInputStream(f);
             ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            byte[] buf = new byte[4096];
            int n;
            while ((n = in.read(buf)) >= 0) {
                out.write(buf, 0, n);
            }
            return out.toByteArray();
        } catch (IOException e) {
            return null;
        }
    }

    private void ensureLoaded() {
        if (cert != null && key != null && pemCertBytes != null) {
            return;
        }
        byte[] uidBytes = readFileIfPresent(new File(filesDir, "uniqueid"));
        String legacyUid = uidBytes != null && uidBytes.length >= 16
                ? new String(uidBytes, 0, 16, StandardCharsets.US_ASCII) : null;
        byte[] certBytes = readFileIfPresent(new File(filesDir, "client.crt"));
        String legacyCertPem = certBytes != null
                ? new String(certBytes, StandardCharsets.US_ASCII) : null;
        byte[] legacyKey = readFileIfPresent(new File(filesDir, "client.key"));

        String raw = NightjarHostsJni.identityLoadOrCreate(
                filesDir.getAbsolutePath(), legacyUid, legacyCertPem, legacyKey);
        Json.Obj reply = Json.parseObject(raw);
        if (!reply.has("ok")) {
            LimeLog.severe("Failed to load client identity: " + reply.optString("error", "unknown"));
            return;
        }
        Json.Obj ok = reply.getObject("ok");
        try {
            byte[] certDer = java.util.Base64.getDecoder().decode(ok.getString("cert_der_b64"));
            byte[] keyPkcs8 = java.util.Base64.getDecoder().decode(ok.getString("key_pkcs8_b64"));
            String certPem = ok.getString("cert_pem");

            CertificateFactory certFactory = CertificateFactory.getInstance("X.509");
            X509Certificate loadedCert = (X509Certificate) certFactory.generateCertificate(
                    new ByteArrayInputStream(certDer));
            KeyFactory keyFactory = KeyFactory.getInstance("RSA");
            PrivateKey loadedKey = keyFactory.generatePrivate(new PKCS8EncodedKeySpec(keyPkcs8));

            cert = loadedCert;
            key = loadedKey;
            pemCertBytes = certPem.replace("\r", "").getBytes(StandardCharsets.US_ASCII);
        } catch (CertificateException | NoSuchAlgorithmException | InvalidKeySpecException
                 | IllegalArgumentException e) {
            throw new RuntimeException(e);
        }
    }

    @Override
    public X509Certificate getClientCertificate() {
        synchronized (globalCryptoLock) {
            ensureLoaded();
            return cert;
        }
    }

    @Override
    public PrivateKey getClientPrivateKey() {
        synchronized (globalCryptoLock) {
            ensureLoaded();
            return key;
        }
    }

    @Override
    public byte[] getPemEncodedClientCertificate() {
        synchronized (globalCryptoLock) {
            ensureLoaded();
            return pemCertBytes;
        }
    }

    @Override
    public String encodeBase64String(byte[] data) {
        return Base64.encodeToString(data, Base64.NO_WRAP);
    }
}
