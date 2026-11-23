/**
 * Real loopback HTTP/1.1 and pinned TLS test server for migrating unit tests onto
 * {@code nightjar_core} JNI HTTP transport without OkHttp reflection.
 */
package com.limelight.nvstream.http;

import com.limelight.nativeapi.NativeApiFixtures;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;

import java.io.ByteArrayOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.math.BigInteger;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Date;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLServerSocket;

public final class TestHttpServer implements AutoCloseable {
    @FunctionalInterface
    public interface Handler {
        Response handle(RecordedRequest request) throws Exception;
    }

    @FunctionalInterface
    public interface RawResponder {
        void write(OutputStream out) throws Exception;
    }

    public static final class RecordedRequest {
        public final String scheme;
        public final String method;
        public final String path;
        public final String rawQuery;
        public final Map<String, String> headers;
        public final byte[] body;

        RecordedRequest(String scheme, String method, String path, String rawQuery,
                        Map<String, String> headers, byte[] body) {
            this.scheme = scheme;
            this.method = method;
            this.path = path;
            this.rawQuery = rawQuery;
            this.headers = Collections.unmodifiableMap(headers);
            this.body = body;
        }

        public String header(String name) {
            return headers.get(name.toLowerCase(Locale.ROOT));
        }

        public String queryParam(String name) {
            List<String> values = queryValues(name);
            return values.isEmpty() ? null : values.get(0);
        }

        public List<String> queryValues(String name) {
            if (rawQuery == null || rawQuery.isEmpty()) {
                return Collections.emptyList();
            }
            List<String> out = new ArrayList<>();
            for (String part : rawQuery.split("&")) {
                if (part.isEmpty()) continue;
                int eq = part.indexOf('=');
                String k = decode(eq < 0 ? part : part.substring(0, eq));
                if (name.equals(k)) {
                    out.add(eq < 0 ? "" : decode(part.substring(eq + 1)));
                }
            }
            return out;
        }

        public String bodyUtf8() {
            return new String(body, StandardCharsets.UTF_8);
        }

        private static String decode(String s) {
            return URLDecoder.decode(s, StandardCharsets.UTF_8);
        }
    }

    public static final class Response {
        final int status;
        final String contentType;
        final byte[] body;
        final Map<String, String> headers = new LinkedHashMap<>();
        boolean chunked;
        long declaredLength = -1;
        long delayMs;
        RawResponder rawResponder;

        private Response(int status, String contentType, byte[] body) {
            this.status = status;
            this.contentType = contentType;
            this.body = body == null ? new byte[0] : body;
        }

        public static Response xml(int status, String xml) {
            return new Response(status, "application/xml; charset=utf-8",
                    xml.getBytes(StandardCharsets.UTF_8));
        }

        public static Response json(int status, String json) {
            return new Response(status, "application/json; charset=utf-8",
                    json.getBytes(StandardCharsets.UTF_8));
        }

        public static Response text(int status, String text) {
            return new Response(status, "text/plain; charset=utf-8",
                    text.getBytes(StandardCharsets.UTF_8));
        }

        public static Response bytes(int status, String contentType, byte[] body) {
            return new Response(status, contentType, body);
        }

        public static Response raw(RawResponder responder) {
            Response r = new Response(200, null, null);
            r.rawResponder = responder;
            return r;
        }

        public Response header(String name, String value) {
            headers.put(name, value);
            return this;
        }

        public Response chunked() {
            this.chunked = true;
            return this;
        }

        public Response declaredContentLength(long len) {
            this.declaredLength = len;
            return this;
        }

        public Response delayMs(long ms) {
            this.delayMs = ms;
            return this;
        }
    }

    private static KeyPair primaryKeys;
    private static X509Certificate primaryCert;
    private static KeyPair secondaryKeys;
    private static X509Certificate secondaryCert;

    public static synchronized void ensureIdentities() {
        java.security.Provider sunEc = java.security.Security.getProvider("SunEC");
        if (sunEc != null) {
            java.security.Security.removeProvider("SunEC");
            java.security.Security.insertProviderAt(sunEc, 1);
        }
        if (primaryCert != null) return;
        try {
            KeyPairGenerator gen = KeyPairGenerator.getInstance("RSA");
            gen.initialize(2048);
            primaryKeys = gen.generateKeyPair();
            secondaryKeys = gen.generateKeyPair();
            X500Name primaryName = new X500Name("CN=Rubylight Test Host");
            X500Name secondaryName = new X500Name("CN=Other Test Host");
            primaryCert = new JcaX509CertificateConverter().getCertificate(
                    new JcaX509v3CertificateBuilder(primaryName, BigInteger.ONE, new Date(0),
                            new Date(4102444800000L), primaryName, primaryKeys.getPublic())
                            .build(new JcaContentSignerBuilder("SHA256withRSA").build(primaryKeys.getPrivate())));
            secondaryCert = new JcaX509CertificateConverter().getCertificate(
                    new JcaX509v3CertificateBuilder(secondaryName, BigInteger.TWO, new Date(0),
                            new Date(4102444800000L), secondaryName, secondaryKeys.getPublic())
                            .build(new JcaContentSignerBuilder("SHA256withRSA").build(secondaryKeys.getPrivate())));
        } catch (Exception e) {
            throw new IllegalStateException(e);
        }
    }

    public static X509Certificate hostCert() {
        ensureIdentities();
        return primaryCert;
    }

    public static PrivateKey hostKey() {
        ensureIdentities();
        return primaryKeys.getPrivate();
    }

    public static X509Certificate otherCert() {
        ensureIdentities();
        return secondaryCert;
    }

    public static String sha256Hex(X509Certificate cert) {
        try {
            return HexFormat.of().formatHex(
                    MessageDigest.getInstance("SHA-256").digest(cert.getEncoded()));
        } catch (Exception e) {
            throw new IllegalStateException(e);
        }
    }

    private final X509Certificate serverCert;
    private final ServerSocket httpSocket;
    private final SSLServerSocket httpsSocket;
    private final Handler handler;
    private final List<RecordedRequest> requests = Collections.synchronizedList(new ArrayList<>());
    private final ExecutorService workers = Executors.newCachedThreadPool(r -> {
        Thread t = new Thread(r, "TestHttpServer");
        t.setDaemon(true);
        return t;
    });
    private volatile boolean closed;

    public TestHttpServer(Handler handler) throws IOException {
        this(hostCert(), hostKey(), handler);
    }

    public TestHttpServer(X509Certificate cert, PrivateKey key, Handler handler) throws IOException {
        this.serverCert = cert;
        this.handler = handler;
        InetAddress loopback = InetAddress.getLoopbackAddress();
        this.httpSocket = new ServerSocket(0, 16, loopback);
        try {
            char[] password = "test".toCharArray();
            KeyStore store = KeyStore.getInstance("PKCS12");
            store.load(null, null);
            store.setKeyEntry("host", key, password, new Certificate[] {cert});
            KeyManagerFactory kmf = KeyManagerFactory.getInstance("SunX509", "SunJSSE");
            kmf.init(store, password);
            SSLContext ssl = SSLContext.getInstance("TLS", "SunJSSE");
            ssl.init(kmf.getKeyManagers(), null, null);
            this.httpsSocket = (SSLServerSocket) ssl.getServerSocketFactory().createServerSocket(0, 16, loopback);
            this.httpsSocket.setNeedClientAuth(false);
        } catch (Exception e) {
            this.httpSocket.close();
            throw new IOException("TLS setup failed", e);
        }
        workers.submit(() -> acceptLoop(httpSocket, "http"));
        workers.submit(() -> acceptLoop(httpsSocket, "https"));
    }

    public int httpPort() {
        return httpSocket.getLocalPort();
    }

    public int httpsPort() {
        return httpsSocket.getLocalPort();
    }

    public X509Certificate serverCert() {
        return serverCert;
    }

    public List<RecordedRequest> requests() {
        synchronized (requests) {
            return new ArrayList<>(requests);
        }
    }

    public NvHTTP newClient() throws IOException {
        return newClient("1234567890abcdef", true, NativeApiFixtures.crypto());
    }

    public NvHTTP newClient(boolean pinServerCert) throws IOException {
        return newClient("1234567890abcdef", pinServerCert, NativeApiFixtures.crypto());
    }

    public NvHTTP newClient(String uniqueId, boolean pinServerCert, LimelightCryptoProvider crypto)
            throws IOException {
        return new NvHTTP(
                new ComputerDetails.AddressTuple("127.0.0.1", httpPort()),
                httpsPort(),
                uniqueId,
                pinServerCert ? serverCert : null,
                crypto);
    }

    private void acceptLoop(ServerSocket server, String scheme) {
        while (!closed && !server.isClosed()) {
            try {
                Socket socket = server.accept();
                workers.submit(() -> serve(socket, scheme));
            } catch (IOException ignored) {
                if (closed || server.isClosed()) break;
            }
        }
    }

    private void serve(Socket socket, String scheme) {
        try (Socket s = socket) {
            s.setTcpNoDelay(true);
            InputStream in = s.getInputStream();
            OutputStream out = s.getOutputStream();
            while (!closed) {
                RecordedRequest req = readRequest(in, out, scheme);
                if (req == null) break;
                requests.add(req);
                Response res = handler.handle(req);
                if (res == null) break;
                if (res.delayMs > 0) {
                    Thread.sleep(res.delayMs);
                }
                if (res.rawResponder != null) {
                    res.rawResponder.write(out);
                    out.flush();
                    break;
                }
                writeResponse(out, res);
                out.flush();
                break;
            }
        } catch (Exception e) {
            if (!closed) {
                e.printStackTrace(System.err);
            }
        }
    }

    private static RecordedRequest readRequest(InputStream in, OutputStream out, String scheme) throws IOException {
        String requestLine = readLineAscii(in);
        if (requestLine == null) return null;
        while (requestLine.isEmpty()) {
            requestLine = readLineAscii(in);
            if (requestLine == null) return null;
        }
        String[] parts = requestLine.split(" ");
        if (parts.length < 2) return null;
        String method = parts[0];
        String target = parts[1];
        int q = target.indexOf('?');
        String path = q < 0 ? target : target.substring(0, q);
        String rawQuery = q < 0 ? "" : target.substring(q + 1);
        Map<String, String> headers = new LinkedHashMap<>();
        String line;
        while ((line = readLineAscii(in)) != null && !line.isEmpty()) {
            int colon = line.indexOf(':');
            if (colon > 0) {
                headers.put(line.substring(0, colon).trim().toLowerCase(Locale.ROOT),
                        line.substring(colon + 1).trim());
            }
        }
        String expect = headers.get("expect");
        if (expect != null && expect.toLowerCase(Locale.ROOT).contains("100-continue")) {
            out.write("HTTP/1.1 100 Continue\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
            out.flush();
        }
        byte[] body;
        String te = headers.get("transfer-encoding");
        String cl = headers.get("content-length");
        if (te != null && te.toLowerCase(Locale.ROOT).contains("chunked")) {
            body = readChunkedBody(in);
        } else if (cl != null) {
            int len = Integer.parseInt(cl);
            body = new byte[len];
            readExact(in, body, len);
        } else {
            body = new byte[0];
        }
        return new RecordedRequest(scheme, method, path, rawQuery, headers, body);
    }

    private static void writeResponse(OutputStream out, Response res) throws IOException {
        StringBuilder head = new StringBuilder();
        head.append("HTTP/1.1 ").append(res.status).append(' ').append(reasonPhrase(res.status)).append("\r\n");
        if (res.contentType != null && !hasHeaderIgnoreCase(res.headers, "Content-Type")) {
            head.append("Content-Type: ").append(res.contentType).append("\r\n");
        }
        for (Map.Entry<String, String> e : res.headers.entrySet()) {
            head.append(e.getKey()).append(": ").append(e.getValue()).append("\r\n");
        }
        if (res.chunked) {
            head.append("Transfer-Encoding: chunked\r\n");
        } else {
            long len = res.declaredLength >= 0 ? res.declaredLength : res.body.length;
            head.append("Content-Length: ").append(len).append("\r\n");
        }
        head.append("Connection: close\r\n\r\n");
        out.write(head.toString().getBytes(StandardCharsets.US_ASCII));
        if (res.chunked) {
            int offset = 0;
            while (offset < res.body.length) {
                int chunk = Math.min(8192, res.body.length - offset);
                out.write((Integer.toHexString(chunk) + "\r\n").getBytes(StandardCharsets.US_ASCII));
                out.write(res.body, offset, chunk);
                out.write("\r\n".getBytes(StandardCharsets.US_ASCII));
                offset += chunk;
            }
            out.write("0\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
        } else if (res.body.length > 0) {
            out.write(res.body);
        }
    }

    private static boolean hasHeaderIgnoreCase(Map<String, String> headers, String target) {
        for (String key : headers.keySet()) {
            if (key.equalsIgnoreCase(target)) return true;
        }
        return false;
    }

    private static String reasonPhrase(int code) {
        switch (code) {
            case 200: return "OK";
            case 201: return "Created";
            case 204: return "No Content";
            case 302: return "Found";
            case 400: return "Bad Request";
            case 403: return "Forbidden";
            case 404: return "Not Found";
            case 409: return "Conflict";
            case 410: return "Gone";
            case 413: return "Payload Too Large";
            case 415: return "Unsupported Media Type";
            case 500: return "Internal Server Error";
            case 503: return "Service Unavailable";
            default: return "Status";
        }
    }

    private static String readLineAscii(InputStream in) throws IOException {
        ByteArrayOutputStream buf = new ByteArrayOutputStream();
        int b;
        boolean any = false;
        while ((b = in.read()) != -1) {
            any = true;
            if (b == '\n') break;
            if (b != '\r') buf.write(b);
        }
        if (!any && buf.size() == 0) return null;
        return buf.toString(StandardCharsets.US_ASCII);
    }

    private static void readExact(InputStream in, byte[] dst, int len) throws IOException {
        int off = 0;
        while (off < len) {
            int n = in.read(dst, off, len - off);
            if (n < 0) throw new EOFException("Unexpected EOF");
            off += n;
        }
    }

    private static byte[] readChunkedBody(InputStream in) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        while (true) {
            String sizeLine = readLineAscii(in);
            if (sizeLine == null) throw new EOFException();
            int semi = sizeLine.indexOf(';');
            int len = Integer.parseInt((semi < 0 ? sizeLine : sizeLine.substring(0, semi)).trim(), 16);
            if (len == 0) {
                readLineAscii(in);
                return out.toByteArray();
            }
            byte[] chunk = new byte[len];
            readExact(in, chunk, len);
            out.write(chunk);
            readLineAscii(in);
        }
    }

    @Override
    public void close() throws IOException {
        closed = true;
        try {
            httpSocket.close();
        } finally {
            try {
                httpsSocket.close();
            } finally {
                workers.shutdownNow();
            }
        }
    }
}
