/**
 * Image clipboard validation errors and local PNG stream/size helpers (OkHttp removed).
 */
package com.limelight.nvstream.http;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Arrays;

public final class ClipboardImageException extends IOException {
    public enum Reason {
        TOO_LARGE,
        NOT_PNG,
        UNSUPPORTED,
        FORBIDDEN,
        NO_HOST_SUPPORT,
    }

    public static final int MAX_BYTES = 64 * 1024 * 1024;
    public static final int MAX_SIDE = 16384;
    public static final long MAX_PIXELS = 1L << 25;
    static final byte[] PNG_SIGNATURE = {(byte) 0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'};
    private static final byte[] IHDR = {0, 0, 0, 13, 'I', 'H', 'D', 'R'};

    public final Reason reason;

    public ClipboardImageException(Reason reason, String message) {
        super(message);
        this.reason = reason;
    }

    public static boolean isPng(byte[] bytes) {
        return bytes != null && bytes.length >= PNG_SIGNATURE.length
                && Arrays.equals(Arrays.copyOf(bytes, PNG_SIGNATURE.length), PNG_SIGNATURE);
    }

    public static int[] pngSize(byte[] png) throws ClipboardImageException {
        return pngSize(png, png == null ? 0 : png.length);
    }

    static int[] pngSize(byte[] png, long length) throws ClipboardImageException {
        if (length > MAX_BYTES) throw new ClipboardImageException(Reason.TOO_LARGE, "Image exceeds 64 MiB limit");
        if (png == null || length < 24 || !isPng(png)
                || !Arrays.equals(Arrays.copyOfRange(png, 8, 16), IHDR)) {
            throw new ClipboardImageException(Reason.NOT_PNG, "Not a valid PNG image");
        }
        long width = Integer.toUnsignedLong(((png[16] & 0xff) << 24) | ((png[17] & 0xff) << 16)
                | ((png[18] & 0xff) << 8) | (png[19] & 0xff));
        long height = Integer.toUnsignedLong(((png[20] & 0xff) << 24) | ((png[21] & 0xff) << 16)
                | ((png[22] & 0xff) << 8) | (png[23] & 0xff));
        if (width == 0 || height == 0) {
            throw new ClipboardImageException(Reason.NOT_PNG, "Not a valid PNG image");
        }
        if (width > MAX_SIDE || height > MAX_SIDE || width * height > MAX_PIXELS) {
            throw new ClipboardImageException(Reason.TOO_LARGE, "PNG dimensions exceed limit");
        }
        return new int[] {(int) width, (int) height};
    }

    public static long copyPng(InputStream in, OutputStream out) throws IOException {
        byte[] buffer = new byte[16 * 1024];
        ByteArrayOutputStream head = new ByteArrayOutputStream(24);
        long total = 0;
        int n;
        while ((n = in.read(buffer)) != -1) {
            total += n;
            if (total > MAX_BYTES) {
                throw new ClipboardImageException(Reason.TOO_LARGE, "Image exceeds 64 MiB limit");
            }
            if (head.size() < 24) {
                int take = Math.min(n, 24 - head.size());
                head.write(buffer, 0, take);
            }
            out.write(buffer, 0, n);
        }
        pngSize(head.toByteArray(), total);
        return total;
    }

    public static int[] pngSize(File file) throws IOException {
        long length = file.length();
        byte[] head = new byte[24];
        try (FileInputStream in = new FileInputStream(file)) {
            int got = 0;
            while (got < 24) {
                int n = in.read(head, got, 24 - got);
                if (n < 0) break;
                got += n;
            }
            if (got < 24) {
                throw new ClipboardImageException(Reason.NOT_PNG, "Truncated PNG header");
            }
        }
        return pngSize(head, length);
    }
}
