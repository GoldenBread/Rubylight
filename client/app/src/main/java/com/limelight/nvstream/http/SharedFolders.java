/**
 * Shared folders browsing and SAF stream transfer client backed by {@code nightjar_core::http::folders}.
 */
package com.limelight.nvstream.http;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

public final class SharedFolders {
    public static final String PART_SUFFIX = ".part";
    public static final int MAX_FOLDER_DEPTH = 64;
    static final int MAX_JSON_BYTES = 1024 * 1024;
    static final int MAX_LIST_BYTES = 4 * 1024 * 1024;

    public static final class Folder {
        public final String id;
        public final String name;
        public final boolean writable;

        Folder(String id, String name, boolean writable) {
            this.id = id;
            this.name = name;
            this.writable = writable;
        }
    }

    public enum Kind { FILE, DIR, UNKNOWN }

    public static final class Entry {
        public final String name;
        public final Kind kind;
        public final long size;
        public final long modifiedUnixMs;

        Entry(String name, Kind kind, long size, long modifiedUnixMs) {
            this.name = name;
            this.kind = kind;
            this.size = size;
            this.modifiedUnixMs = modifiedUnixMs;
        }
    }

    public static final class Listing {
        public final String path;
        public final List<Entry> entries;

        Listing(String path, List<Entry> entries) {
            this.path = path;
            this.entries = Collections.unmodifiableList(entries);
        }
    }

    public interface Progress {
        void onProgress(long done, long total);
    }

    public enum Refusal {
        FORBIDDEN(403, "forbidden"),
        FOLDERS_DISABLED(403, "folders_disabled"),
        FOLDER_UNREADABLE(403, "folder_unreadable"),
        NOT_FOUND(404, "not_found"),
        INVALID_NAME(400, "invalid_name"),
        INCOMPLETE_UPLOAD(400, "incomplete_upload"),
        LENGTH_REQUIRED(411, "length_required"),
        TOO_LARGE(413, "too_large"),
        INSUFFICIENT_STORAGE(507, "insufficient_storage"),
        NO_CONSOLE_USER(409, "no_console_user"),
        TRANSFER_BUSY(409, "transfer_busy");

        final int status;
        final String code;

        Refusal(int status, String code) {
            this.status = status;
            this.code = code;
        }
    }

    public static final class RefusedException extends IOException {
        public final Refusal refusal;
        public final int status;

        RefusedException(Refusal refusal, String message) {
            super(message);
            this.refusal = refusal;
            this.status = refusal.status;
        }
    }

    public static final class LocalFileException extends IOException {
        public LocalFileException(IOException cause) {
            super(cause.getMessage(), cause);
        }
    }

    public static final class InvalidPathException extends IllegalArgumentException {
        InvalidPathException(String message) {
            super(message);
        }
    }

    public static final class CancelledException extends IOException {
        CancelledException() {
            super("Transfer cancelled");
        }
    }

    private final NvHTTP http;

    public SharedFolders(NvHTTP http) {
        this.http = http;
    }

    public List<Folder> folders() throws IOException {
        try {
            Json.Obj reply = check(NightjarHttpJni.folderList(http.httpHandle(), null));
            Json.Arr arr = reply.getArray("ok");
            List<Folder> out = new ArrayList<>(arr.length());
            for (int i = 0; i < arr.length(); i++) {
                Json.Obj item = arr.getObject(i);
                out.add(new Folder(item.getString("id"), item.getString("name"), item.getBoolean("writable")));
            }
            return out;
        } catch (Json.SyntaxException e) {
            throw new IOException("Malformed shared folders list", e);
        }
    }

    public Listing entries(String id, String path) throws IOException {
        checkId(id);
        checkPath(path);
        try {
            Json.Obj reply = check(NightjarHttpJni.folderEntries(http.httpHandle(), null, id, path == null ? "" : path));
            Json.Obj ok = reply.getObject("ok");
            Json.Arr arr = ok.getArray("entries");
            List<Entry> entries = new ArrayList<>(arr.length());
            for (int i = 0; i < arr.length(); i++) {
                Json.Obj e = arr.getObject(i);
                Kind kind = "DIRECTORY".equals(e.getString("kind")) ? Kind.DIR
                        : "FILE".equals(e.getString("kind")) ? Kind.FILE : Kind.UNKNOWN;
                entries.add(new Entry(e.getString("name"), kind, e.getLong("size"), e.getLong("modified_unix_ms")));
            }
            return new Listing(ok.getString("path"), entries);
        } catch (Json.SyntaxException e) {
            throw new IOException("Malformed folder listing", e);
        }
    }

    public long download(String id, String path, OutputStream sink, Progress progress, TransferCancel cancel)
            throws IOException {
        return download(id, path, -1, sink, progress, cancel);
    }

    public long download(String id, String path, long listed, OutputStream sink, Progress progress,
                         TransferCancel cancel) throws IOException {
        checkId(id);
        checkPath(path);
        if (path == null || path.isEmpty()) throw new InvalidPathException("A download needs a file path");
        IOException[] localError = new IOException[1];
        NightjarHttpJni.ByteSink jniSink = (buf, off, len) -> {
            try {
                sink.write(buf, off, len);
            } catch (IOException e) {
                localError[0] = e;
                throw e;
            }
        };
        NightjarHttpJni.FolderProgress jniProgress = progress == null ? null : progress::onProgress;
        long cancelHandle = cancel == null ? 0 : cancel.handle();
        try {
            Json.Obj reply = checkDownloadOrUpload(
                    NightjarHttpJni.folderDownload(http.httpHandle(), null, id, path, listed, jniSink, jniProgress, cancelHandle),
                    localError[0]);
            return reply.getLong("ok");
        } catch (Json.SyntaxException e) {
            throw new IOException("Malformed download reply", e);
        }
    }

    public String upload(String id, String path, InputStream file, long length, Progress progress,
                         TransferCancel cancel) throws IOException {
        checkId(id);
        checkPath(path);
        if (path == null || path.isEmpty()) throw new InvalidPathException("An upload needs a file path");
        if (hasPartSuffix(fileName(path))) throw new InvalidPathException("Temporary .part names are reserved");
        if (length < 0) throw new LocalFileException(new IOException("Negative upload length"));
        IOException[] localError = new IOException[1];
        NightjarHttpJni.ByteSource jniSource = (buf, off, len) -> {
            try {
                return file.read(buf, off, len);
            } catch (IOException e) {
                localError[0] = e;
                throw e;
            }
        };
        NightjarHttpJni.FolderProgress jniProgress = progress == null ? null : progress::onProgress;
        long cancelHandle = cancel == null ? 0 : cancel.handle();
        try {
            Json.Obj reply = checkDownloadOrUpload(
                    NightjarHttpJni.folderUpload(http.httpHandle(), null, id, path, jniSource, length, jniProgress, cancelHandle),
                    localError[0]);
            return reply.getString("ok");
        } catch (Json.SyntaxException e) {
            throw new IOException("Malformed upload reply", e);
        }
    }

    private static Json.Obj check(String raw) throws IOException {
        return checkDownloadOrUpload(raw, null);
    }

    private static Json.Obj checkDownloadOrUpload(String raw, IOException localCause) throws IOException {
        try {
            Json.Obj reply = Json.parseObject(raw);
            if (!reply.has("error")) return reply;
            Json.Obj err = reply.getObject("error");
            String kind = err.optString("kind");
            String msg = err.optString("message", "Shared folder error");
            if ("local_file".equals(kind)) {
                throw new LocalFileException(localCause != null ? localCause : new IOException(msg));
            }
            if ("cancelled".equals(kind)) throw new CancelledException();
            if ("refused".equals(kind)) {
                Refusal refusal;
                try { refusal = Refusal.valueOf(err.getString("refusal")); }
                catch (IllegalArgumentException e) { throw new IOException("Unknown refusal: " + msg, e); }
                throw new RefusedException(refusal, msg);
            }
            if ("invalid_argument".equals(kind)) throw new InvalidPathException(msg);
            if ("status".equals(kind)) {
                int code = err.has("error_code") ? err.getInt("error_code") : err.optInt("http_status", 500);
                throw new HostHttpResponseException(code, err.optString("error_message", msg));
            }
            if (err.has("http_status") && err.getInt("http_status") >= 400) {
                throw new HostHttpResponseException(err.getInt("http_status"), msg);
            }
            throw new IOException(msg);
        } catch (Json.SyntaxException e) {
            throw new IOException("Malformed folder error response", e);
        }
    }

    static void checkId(String id) {
        if (id == null || id.isEmpty() || id.length() > 128) throw new InvalidPathException("Invalid folder ID");
        for (int i = 0; i < id.length(); i++) {
            char c = id.charAt(i);
            if (!((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-' || c == '_')) {
                throw new InvalidPathException("Invalid folder ID");
            }
        }
    }

    static void checkPath(String path) {
        if (path == null || path.isEmpty()) return;
        if (path.startsWith("/") || path.endsWith("/") || path.indexOf('\\') >= 0 || path.indexOf(':') >= 0) {
            throw new InvalidPathException("Not a shared folder path: \"" + path + "\"");
        }
        String[] parts = path.split("/", -1);
        if (parts.length > MAX_FOLDER_DEPTH) throw new InvalidPathException("Not a shared folder path: \"" + path + "\"");
        for (String part : parts) {
            if (part.isEmpty() || ".".equals(part) || "..".equals(part)) {
                throw new InvalidPathException("Not a shared folder path: \"" + path + "\"");
            }
        }
    }

    public static String childPath(String dir, String name) {
        return dir == null || dir.isEmpty() ? name : dir + "/" + name;
    }

    public static String parentPath(String path) {
        int slash = path == null ? -1 : path.lastIndexOf('/');
        return slash < 0 ? "" : path.substring(0, slash);
    }

    public static String fileName(String path) {
        if (path == null) return "";
        int slash = path.lastIndexOf('/');
        return slash < 0 ? path : path.substring(slash + 1);
    }

    public static boolean hasPartSuffix(String name) {
        return name != null && name.toLowerCase(java.util.Locale.ROOT).endsWith(PART_SUFFIX);
    }
}
