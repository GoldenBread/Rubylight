package com.limelight.nvstream.http;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.file.Files;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

/** The host's image clipboard limits, checked on both sides of the transfer over nightjar-core JNI. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ClipboardImageExceptionTest {
    /** A PNG signature and IHDR chunk of the given size; enough for the header checks. */
    static byte[] png(long width, long height) {
        ByteBuffer png = ByteBuffer.allocate(33);
        png.put(ClipboardImageException.PNG_SIGNATURE);
        png.putInt(13).put(new byte[] {'I', 'H', 'D', 'R'});
        png.putInt((int) width).putInt((int) height);
        return png.array();
    }

    private static ClipboardImageException.Reason reason(byte[] png) {
        return assertThrows(ClipboardImageException.class, () -> ClipboardImageException.pngSize(png)).reason;
    }

    @Test
    public void aPngWithinTheHostLimitsGivesItsSize() throws IOException {
        assertArrayEquals(new int[] {1920, 1080}, ClipboardImageException.pngSize(png(1920, 1080)));
        assertArrayEquals(new int[] {16384, 2048}, ClipboardImageException.pngSize(png(16384, 2048)));
        assertTrue(ClipboardImageException.isPng(png(1, 1)));
    }

    @Test
    public void anImageOverASideOrThePixelLimitIsTooLarge() {
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(16385, 1)));
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(1, 16385)));
        // 8192 x 8192 is 64 megapixels: within each side, over the 32 megapixels.
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(8192, 8192)));
        // The width is unsigned: 0xffffffff is not -1.
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, reason(png(0xffffffffL, 1)));
    }

    @Test
    public void anythingButAPngHeaderIsNotAPng() {
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(png(0, 10)));
        byte[] jpeg = png(10, 10);
        jpeg[1] = 'J';
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(jpeg));
        assertFalse(ClipboardImageException.isPng(jpeg));
        byte[] noHeader = png(10, 10);
        noHeader[12] = 'X';
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(noHeader));
        assertEquals(ClipboardImageException.Reason.NOT_PNG, reason(ClipboardImageException.PNG_SIGNATURE));
        assertFalse(ClipboardImageException.isPng(new byte[3]));
    }

    @Test
    public void eachDocumentedStatusIsItsReasonAndOthersStayHttpErrors() throws Exception {
        byte[] valid = png(4, 4);
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>(
                TestHttpServer.Response.bytes(200, "image/png", valid));
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP http = server.newClient();
            assertTrue(http.getClipboardImage(new ByteArrayOutputStream()));
            int[] statuses = {400, 403, 413, 415};
            ClipboardImageException.Reason[] reasons = {ClipboardImageException.Reason.NOT_PNG,
                    ClipboardImageException.Reason.FORBIDDEN, ClipboardImageException.Reason.TOO_LARGE,
                    ClipboardImageException.Reason.UNSUPPORTED};
            for (int i = 0; i < statuses.length; i++) {
                reply.set(TestHttpServer.Response.text(statuses[i], "x"));
                assertEquals(reasons[i], assertThrows(ClipboardImageException.class,
                        () -> http.getClipboardImage(new ByteArrayOutputStream())).reason);
            }
            reply.set(TestHttpServer.Response.text(500, "Server Error"));
            HostHttpResponseException other = assertThrows(HostHttpResponseException.class,
                    () -> http.getClipboardImage(new ByteArrayOutputStream()));
            assertEquals(500, other.getErrorCode());
        }
    }

    @Test
    public void aReceivedImageMustBeAPngOfTypeImagePng() throws Exception {
        byte[] image = png(4, 4);
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP http = server.newClient();
            reply.set(TestHttpServer.Response.bytes(200, "image/png", image));
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            assertTrue(http.getClipboardImage(out));
            assertArrayEquals(image, out.toByteArray());

            reply.set(TestHttpServer.Response.bytes(200, "application/octet-stream", image));
            assertThrows(IOException.class, () -> http.getClipboardImage(new ByteArrayOutputStream()));

            reply.set(TestHttpServer.Response.bytes(200, "text/html", image));
            assertThrows(IOException.class, () -> http.getClipboardImage(new ByteArrayOutputStream()));

            reply.set(TestHttpServer.Response.bytes(200, "image/png", "<html>".getBytes()));
            assertEquals(ClipboardImageException.Reason.NOT_PNG, assertThrows(ClipboardImageException.class,
                    () -> http.getClipboardImage(new ByteArrayOutputStream())).reason);

            reply.set(TestHttpServer.Response.bytes(200, "image/png", new byte[ClipboardImageException.MAX_BYTES + 1]));
            assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                    () -> http.getClipboardImage(new ByteArrayOutputStream())).reason);
        }
    }

    private static byte[] endlessBytes(int length) {
        byte[] head = png(4, 4);
        byte[] data = new byte[length];
        System.arraycopy(head, 0, data, 0, Math.min(head.length, length));
        return data;
    }

    @Test
    public void aReceivedImageOfUnknownLengthIsStreamedAndCutAtTheLimit() throws Exception {
        long[] kept = {0};
        OutputStream counting = new OutputStream() {
            @Override public void write(int b) { kept[0]++; }
            @Override public void write(byte[] b, int off, int len) { kept[0] += len; }
        };
        AtomicReference<TestHttpServer.Response> reply = new AtomicReference<>();
        try (TestHttpServer server = new TestHttpServer(req -> reply.get())) {
            NvHTTP http = server.newClient();
            reply.set(TestHttpServer.Response.bytes(200, "image/png",
                    endlessBytes(ClipboardImageException.MAX_BYTES + 1)).chunked());
            assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                    () -> http.getClipboardImage(counting)).reason);
            assertTrue("wrote " + kept[0], kept[0] <= ClipboardImageException.MAX_BYTES);

            kept[0] = 0;
            reply.set(TestHttpServer.Response.bytes(200, "image/png",
                    endlessBytes(ClipboardImageException.MAX_BYTES)).chunked());
            assertTrue(http.getClipboardImage(counting));
            assertEquals(ClipboardImageException.MAX_BYTES, kept[0]);
        }
    }

    @Test
    public void aFileIsCheckedByItsHeaderAndLength() throws IOException {
        File file = File.createTempFile("clipboard", ".png");
        try {
            Files.write(file.toPath(), png(64, 32));
            assertArrayEquals(new int[] {64, 32}, ClipboardImageException.pngSize(file));
            Files.write(file.toPath(), png(16385, 1));
            assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                    () -> ClipboardImageException.pngSize(file)).reason);
            Files.write(file.toPath(), new byte[] {1, 2, 3});
            assertEquals(ClipboardImageException.Reason.NOT_PNG, assertThrows(ClipboardImageException.class,
                    () -> ClipboardImageException.pngSize(file)).reason);
        } finally {
            assertTrue(file.delete());
        }
    }
}
