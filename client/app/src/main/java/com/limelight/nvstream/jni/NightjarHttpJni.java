/**
 * Pure-JDK JNI declarations for {@code nightjar-core::http} (`protocol/android/src/http.rs` and
 * `http_folders.rs`). Imports only {@code java.*} so Robolectric loads it without Android shadow
 * acquisition; loads {@code nightjar_core} via {@link NightjarJni#ensureLibraryLoaded()}.
 */
package com.limelight.nvstream.jni;

import java.io.IOException;

public final class NightjarHttpJni {
    static {
        NightjarJni.ensureLibraryLoaded();
    }

    private NightjarHttpJni() {
    }

    /** Binary download target passed to {@link #getClipboardImage} and {@link #folderDownload}. */
    public interface ByteSink {
        void write(byte[] buffer, int offset, int length) throws IOException;
    }

    /** Binary upload source passed to {@link #folderUpload}. */
    public interface ByteSource {
        int read(byte[] buffer, int offset, int length) throws IOException;
    }

    /** Percentage callback (0..100) passed to {@link #probeBandwidth} and {@link #probeFoundationBandwidth}. */
    public interface PercentProgress {
        void accept(int percent);
    }

    /** Byte-level transfer progress passed to {@link #folderDownload} and {@link #folderUpload}. */
    public interface FolderProgress {
        void onProgress(long done, long total);
    }

    // HostClient & TransferCancel lifecycle
    public static native void setAppVersion(String version);
    public static native long clientCreate(String address, int httpPort, int httpsPort,
                                           String uniqueId, byte[] clientCertDer,
                                           byte[] clientKeyPkcs8, byte[] serverCertDer,
                                           String deviceName);
    public static native void clientDestroy(long handle);
    public static native void clientCancelPendingRequests(long handle);
    public static native void clientSetHttpsPort(long handle, int port);
    public static native int clientGetHttpsPort(long handle);
    public static native void clientSetServerCertDer(long handle, byte[] certDer);
    public static native byte[] clientGetServerCertDer(long handle);
    public static native void setSecondScreen(boolean secondScreen);

    public static native long transferCancelCreate();
    public static native void transferCancelDestroy(long handle);
    public static native void transferCancelCancel(long handle);
    public static native boolean transferCancelIsCancelled(long handle);

    // ServerInfo, Pairing, AppList
    public static native String getServerInfo(long handle, boolean likelyOnline);
    public static native String probePairedComputer(long handle);
    public static native String parseServerInfoXml(String xml, int httpPort, boolean strict);
    public static native String pair(long handle, String serverInfoXml, String pin,
                                     String passphrase, byte[] expectedCertSha256);
    public static native String unpair(long handle);
    public static native String getAppListRaw(long handle);
    public static native String parseAppListXml(String xml, String capsJson);
    public static native byte[] getBoxArt(long handle, int appId) throws IOException;
    public static native String getBoxArtUrl(long handle, int appId) throws IOException;

    // Launch, Quit, Role Disconnect, Bitrate
    public static native byte[] randomRiKey();
    public static native String buildLaunchQuery(String requestJson, String capsJson, boolean launchByUuid);
    public static native String launchApp(long handle, String verb, String requestJson,
                                          String capsJson, boolean nvidia, int readTimeoutMs);
    public static native String disconnectRole(long handle, int roleOrdinal);
    public static native String quitApp(long handle);
    public static native String launchConfirmationClassify(int status, String body);

    // Bitrate, Clipboard, Image Clipboard, Metrics, Route, InternetPlay
    public static native String setBitrate(long handle, int bitrateKbps);
    public static native String getClipboard(long handle);
    public static native String sendClipboard(long handle, String text);
    public static native String getClipboardImage(long handle, ByteSink sink);
    public static native String sendClipboardImageFile(long handle, String path);
    public static native String getHardwareMetrics(long handle, String capsJson);
    public static native String parseHardwareMetrics(String rawJson);
    public static native String getNetworkPath(long handle, String capsJson);
    public static native String parseNetworkPath(String rawJson);
    public static native String getInternetPlay(long handle, String capsJson);

    // Foundation ABR & Probes
    public static native String supportsFoundationAbr(long handle);
    public static native String configureFoundationAbr(long handle, String reqJson);
    public static native String sendFoundationAbrFeedback(long handle, String fbJson);
    public static native String probeBandwidth(long handle, int bytes, PercentProgress listener);
    public static native String supportsFoundationBandwidthTest(long handle);
    public static native String probeFoundationBandwidth(long handle, PercentProgress listener);
    public static native String probeLinkMbps(long handle);
    public static native String probePyroWaveUdp(long handle, int kbps, int packetSize,
                                                 int fps, int paceKbps);
    public static native String probeServerLatency(long handle, int timeoutMs);

    // SharedFolders
    public static native String folderList(long handle, String capsJson);
    public static native String folderEntries(long handle, String capsJson, String folder, String path);
    public static native String folderDownload(long handle, String capsJson, String folder, String path,
                                               long listed, ByteSink sink, FolderProgress progress,
                                               long cancelHandle);
    public static native String folderUpload(long handle, String capsJson, String folder, String path,
                                             ByteSource source, long length, FolderProgress progress,
                                             long cancelHandle);
}
