package com.limelight.computers;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.SystemClock;

import com.limelight.AppView;
import com.limelight.LimeLog;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.wol.WakeTargetsPoll;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.HostStreamProfile;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.FrontendExportStore;
import com.limelight.utils.NetHelper;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.IOException;
import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;
import java.util.HashMap;

/**
 * Helper codec and Android-specific cleanup/STUN/internet-play utilities for {@link ComputerManagerService}.
 */
final class HostServiceHelpers {
    private HostServiceHelpers() {
    }

    static byte[] certDer(X509Certificate cert) {
        if (cert == null) return null;
        try {
            return cert.getEncoded();
        } catch (CertificateEncodingException e) {
            return null;
        }
    }

    static String slotsJson(ComputerDetails details) throws JSONException {
        JSONObject obj = new JSONObject();
        putSlot(obj, "local", details.localAddress);
        putSlot(obj, "manual", details.manualAddress);
        putSlot(obj, "ipv6", details.ipv6Address);
        putSlot(obj, "remote", details.remoteAddress);
        putSlot(obj, "pinned", details.pinnedAddress);
        return obj.toString();
    }

    private static void putSlot(JSONObject obj, String key, ComputerDetails.AddressTuple tuple)
            throws JSONException {
        if (tuple == null) return;
        JSONObject s = new JSONObject();
        s.put("address", tuple.address);
        s.put("port", tuple.port);
        obj.put(key, s);
    }

    static boolean pollComputerDirect(long hostManagerHandle, ComputerDetails details) {
        try {
            String raw = NightjarHostsJni.hostManagerPollDirect(
                    hostManagerHandle,
                    details.uuid,
                    slotsJson(details),
                    details.state == ComputerDetails.State.ONLINE,
                    details.activeAddress != null ? details.activeAddress.port : 0,
                    details.httpsPort,
                    certDer(details.serverCert));
            if (raw != null) {
                JSONObject res = new JSONObject(raw);
                if (res.optBoolean("reached")) {
                    applyPollResult(details, res);
                    return true;
                }
                if (res.optBoolean("another_certificate")) {
                    return false;
                }
            }
        } catch (JSONException e) {
            LimeLog.warning("Poll direct failed: " + e.getMessage());
        }
        return false;
    }

    static void applyPollResult(ComputerDetails details, JSONObject res) {
        details.state = ComputerDetails.State.ONLINE;
        details.unreachable = null;
        String name = res.optString("name", "");
        if (!name.isEmpty() && !"UNKNOWN".equals(name)) {
            details.name = name;
        }
        String uuid = res.optString("uuid", "");
        if (!uuid.isEmpty()) {
            details.uuid = uuid;
        }
        String mac = res.optString("mac", "");
        if (!mac.isEmpty()) {
            details.macAddress = mac;
        }
        int httpsPort = res.optInt("https_port", 0);
        if (httpsPort > 0) {
            details.httpsPort = httpsPort;
        }
        int extPort = res.optInt("external_port", 0);
        if (extPort > 0) {
            details.externalPort = extPort;
        }
        details.runningGameId = res.optInt("current_game", 0);
        details.runningGameUuid = res.isNull("current_game_uuid") ? null : res.optString("current_game_uuid", null);
        details.pairState = res.optBoolean("paired")
                ? PairingManager.PairState.PAIRED : PairingManager.PairState.NOT_PAIRED;
        if (!res.isNull("capabilities_json")) {
            details.caps = HostCapabilities.fromJson(res.optString("capabilities_json"));
        }
        JSONObject activeObj = res.optJSONObject("active");
        if (activeObj != null) {
            details.activeAddress = new ComputerDetails.AddressTuple(
                    activeObj.optString("address"), activeObj.optInt("port", NvHTTP.DEFAULT_HTTP_PORT));
        }
        String extIp = res.isNull("external_ip") ? null : res.optString("external_ip", null);
        if (extIp != null && !extIp.isEmpty()) {
            details.remoteAddress = new ComputerDetails.AddressTuple(extIp, details.guessExternalPort());
        }
    }

    static boolean pollOverTheInternet(Context context, long hostManagerHandle, ComputerDetails details)
            throws InterruptedException {
        details.unreachable = null;
        String reply = InternetPlay.reply(context, details.uuid);
        if (reply == null) {
            return false;
        }
        InternetPlay.Tried tried = new InternetPlay.Tried();
        tried.lan = details.localAddress != null || details.manualAddress != null;
        InternetPlay.Addresses addresses = InternetPlay.addresses(reply);
        for (ComputerDetails.AddressTuple addr : addresses.ipv6) {
            tried.ipv6 = true;
            if (pollSingleCandidate(hostManagerHandle, details, addr)) {
                return true;
            }
        }
        if (addresses.mapped != null) {
            tried.mapped = true;
            if (pollSingleCandidate(hostManagerHandle, details, addresses.mapped)) {
                return true;
            }
        }
        tried.ipv6 |= details.ipv6Address != null;
        tried.mapped |= details.remoteAddress != null;
        if (Thread.currentThread().isInterrupted()) {
            throw new InterruptedException();
        }
        if (!ComputerManagerService.punchesNow(details)) {
            details.unreachable = InternetPlay.unreachable(reply, tried);
            return false;
        }
        try {
            ComputerDetails.AddressTuple punched = InternetPlay.connect(details.uuid, reply, tried);
            if (punched != null && pollSingleCandidate(hostManagerHandle, details, punched)) {
                return true;
            }
            details.unreachable = InternetPlay.unreachable(reply, tried);
        } catch (InternetPlay.Unreachable e) {
            details.unreachable = e.getMessage();
        }
        return false;
    }

    private static boolean pollSingleCandidate(
            long hostManagerHandle, ComputerDetails details, ComputerDetails.AddressTuple addr) {
        if (addr == null) return false;
        String raw = NightjarHostsJni.hostManagerPollIp(
                hostManagerHandle, details.uuid, addr.address, addr.port,
                details.httpsPort, certDer(details.serverCert),
                details.state == ComputerDetails.State.ONLINE);
        if (raw == null) return false;
        try {
            JSONObject res = new JSONObject(raw);
            if (!res.optBoolean("reached")) return false;
            applyPollResult(details, res);
            return true;
        } catch (JSONException e) {
            return false;
        }
    }

    static void populateExternalAddress(Context context, ComputerDetails details) {
        if (NetHelper.isActiveNetworkVpn(context)) {
            Network lan = stunNetwork((ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE));
            if (lan == null) {
                LimeLog.info("STUN: no non-VPN network to ask on");
                return;
            }
            try {
                String stunResolvedAddress = StunClient.query("stun.moonlight-stream.org", 3478, 3000,
                        lan::getAllByName, lan::bindSocket);
                if (stunResolvedAddress != null) {
                    details.remoteAddress = new ComputerDetails.AddressTuple(
                            stunResolvedAddress, details.guessExternalPort());
                }
            } catch (IOException e) {
                LimeLog.warning("STUN on the non-VPN network failed: " + e);
            }
            return;
        }
        String raw = NightjarHostsJni.hostManagerRefreshExternalIpv4Direct(details.guessExternalPort());
        if (raw != null) {
            try {
                JSONObject slot = new JSONObject(raw);
                details.remoteAddress = new ComputerDetails.AddressTuple(
                        slot.getString("address"), slot.getInt("port"));
            } catch (JSONException ignored) {
            }
        }
    }

    private static Network stunNetwork(ConnectivityManager connMgr) {
        for (Network net : connMgr.getAllNetworks()) {
            NetworkCapabilities netCaps = connMgr.getNetworkCapabilities(net);
            if (netCaps != null && !netCaps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)
                    && !netCaps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
                return net;
            }
        }
        return null;
    }

    static void readInternetPlay(
            Context context,
            IdentityManager idManager,
            HashMap<String, Long> internetReads,
            java.util.concurrent.ExecutorService executor,
            ComputerDetails details) {
        if (details.state != ComputerDetails.State.ONLINE
                || details.pairState != PairingManager.PairState.PAIRED
                || details.serverCert == null || details.uuid == null
                || !details.caps.has(HostCapabilities.Feature.INTERNET_PLAY)
                || !"lan".equals(details.internetPath)) {
            return;
        }
        long now = SystemClock.elapsedRealtime();
        synchronized (internetReads) {
            if (!InternetPlay.due(internetReads.get(details.uuid), now)) {
                return;
            }
            internetReads.put(details.uuid, now);
        }
        final String uuid = details.uuid;
        final ComputerDetails.AddressTuple address = details.activeAddress;
        final int httpsPort = details.httpsPort;
        final X509Certificate cert = details.serverCert;
        try {
            executor.execute(() -> {
                try {
                    NvHTTP http = new NvHTTP(address, httpsPort, idManager.getUniqueId(), cert,
                            com.limelight.binding.PlatformBinding.getCryptoProvider(context));
                    InternetPlay.save(context, uuid, http.getInternetPlay());
                } catch (IOException e) {
                    LimeLog.warning("Internet play: the PC's details could not be read: " + e.getMessage());
                }
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
        }
    }

    static void cleanupRemovedComputer(
            Context context,
            ComputerDatabaseManager dbManager,
            WakeTargetsPoll wakeTargets,
            ComputerDetails computer) {
        dbManager.deleteComputer(computer);
        wakeTargets.forget(computer.uuid);
        HostStreamProfile.reset(context, computer.uuid);
        HostDisplayChoice.forget(context, computer.uuid);
        HostCapabilityCache.forget(context, computer.uuid);
        InternetPlay.forget(context, computer.uuid);
        com.limelight.nativeapi.ConsoleRepository.forgetHost(context, computer.uuid);
        HostNotices.forget(context, computer.uuid);
        HostNames.forget(context, computer.uuid);
        PinnedAddresses.set(context, computer.uuid, null);
    }

    static void cleanupRemovedComputerPrefs(Context context, String uuid) {
        com.limelight.preferences.AppQuitOnLeave.forget(context, uuid);
        com.limelight.preferences.AppStreamSettings.forget(context, uuid);
        com.limelight.preferences.AutoBitrateChoice.forget(context, uuid);
        context.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, Context.MODE_PRIVATE).edit()
                .remove(uuid)
                .remove(com.limelight.nativeapi.HiddenStore.Preferences.idsKey(uuid)).apply();
        FrontendExportStore.forgetHost(context, uuid);
        CacheHelper.deleteCacheFile(context.getCacheDir(), "applist", uuid);
        new DiskAssetLoader(context).deleteAssetsForComputer(uuid);
    }

    static boolean saveInvitedComputerInDb(
            Context context,
            ComputerDatabaseManager dbManager,
            ComputerDetails details,
            String reply) {
        ComputerDetails saved = dbManager.getComputerByUUID(details.uuid);
        if (saved != null && saved.serverCert != null && !saved.serverCert.equals(details.serverCert)) {
            return false;
        }
        InternetPlay.save(context, details.uuid, reply);
        if (saved == null) {
            dbManager.updateComputer(details);
        } else {
            saved.name = details.name;
            saved.serverCert = details.serverCert;
            saved.httpsPort = details.httpsPort;
            if (details.manualAddress != null) saved.manualAddress = details.manualAddress;
            dbManager.updateComputer(saved);
        }
        return true;
    }

    static void forgetCertInDb(ComputerDatabaseManager dbManager, String uuid) {
        ComputerDetails saved = dbManager.getComputerByUUID(uuid);
        if (saved != null && saved.serverCert != null) {
            saved.serverCert = null;
            dbManager.updateComputer(saved);
        }
    }

    static ComputerDetails detailsFromMdns(com.limelight.nvstream.mdns.MdnsComputer computer, Context context) {
        ComputerDetails details = new ComputerDetails();
        if (computer.getLocalAddress() != null) {
            details.localAddress = new ComputerDetails.AddressTuple(
                    computer.getLocalAddress().getHostAddress(), computer.getPort());
            if (computer.getLocalAddress() instanceof java.net.Inet4Address) {
                populateExternalAddress(context, details);
            }
        }
        if (computer.getIpv6Address() != null) {
            details.ipv6Address = new ComputerDetails.AddressTuple(
                    computer.getIpv6Address().getHostAddress(), computer.getPort());
        }
        return details;
    }

    static ComputerDetails.AddressTuple currentAddressOrNull(ComputerDetails computer) {
        try {
            return com.limelight.utils.ServerHelper.getCurrentAddressFromComputer(computer);
        } catch (IOException e) {
            return null;
        }
    }

    static void saveAppListFile(Context context, String uuid, String appList) {
        try (java.io.OutputStream cacheOut = CacheHelper.openCacheFileForOutput(
                context.getCacheDir(), "applist", uuid)) {
            CacheHelper.writeStringToOutputStream(cacheOut, appList);
        } catch (IOException e) {
            e.printStackTrace();
        }
    }
}
