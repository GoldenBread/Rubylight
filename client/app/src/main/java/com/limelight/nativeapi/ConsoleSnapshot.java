package com.limelight.nativeapi;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.Json;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/** What Home shows: every title of every paired host, and the games to come back to. */
public final class ConsoleSnapshot {
    /** An entry returned by {@code POST /api/native/v1/library/add} or parsed via Rust. */
    public static final class AddedGame {
        public final String installId;
        public final int appId;
        public final String title;
        public final String gameId;
        public final boolean added;
        public final String coverPath;
        public final String coverVersion;
        public final String heroPath;
        public final String logoPath;
        public final String landscapePath;

        public AddedGame(String installId, int appId, String title, String gameId, boolean added,
                         String coverPath, String coverVersion, String heroPath, String logoPath,
                         String landscapePath) {
            this.installId = installId;
            this.appId = appId;
            this.title = title;
            this.gameId = gameId;
            this.added = added;
            this.coverPath = coverPath;
            this.coverVersion = coverVersion;
            this.heroPath = heroPath;
            this.logoPath = logoPath;
            this.landscapePath = landscapePath;
        }

        public static AddedGame fromJson(Json.Obj json) {
            if (json == null) return null;
            Json.Obj art = json.optObject("artwork");
            return new AddedGame(
                    json.isNull("install_id") ? null : json.optString("install_id", null),
                    json.optInt("app_id", 0),
                    json.optString("title", ""),
                    json.isNull("game_id") ? null : json.optString("game_id", null),
                    json.optBoolean("added", true),
                    art == null || art.isNull("cover") ? null : art.optString("cover", null),
                    art == null ? "" : art.optString("version", ""),
                    art == null || art.isNull("hero") ? null : art.optString("hero", null),
                    art == null || art.isNull("logo") ? null : art.optString("logo", null),
                    art == null || art.isNull("landscape") ? null : art.optString("landscape", null));
        }
    }
    /** Where a host's titles came from. */
    public enum Status {
        /** Read from the native API just now. */
        LIVE,
        /** The last native API library, because the host is offline or did not answer. */
        CACHED,
        /** The classic Moonlight app list, for a host without the native API. */
        CLASSIC,
        /** Nothing to show: no cache yet, or no app list. */
        UNAVAILABLE
    }

    /** One title on one host. */
    public static final class Source {
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        /** The native API's id of the title on the host; null for a classic app. */
        public final String installId;
        public final String title;
        /** The native API's id of the title across hosts; null when unknown and for classic apps. */
        public final String gameId;
        public final boolean running;
        /** The cover's path on the host and its version; null for a classic app. */
        public final String coverPath;
        public final String coverVersion;
        /** Null when unknown. */
        public final Boolean installed;
        /** The classic app's UUID, empty otherwise. */
        public final String appUuid;
        public final boolean hdrSupported;
        /** The store's key ("steam", ...) and display name; null for an app no store owns and for classic apps. */
        public final String storeSource;
        public final String storeName;
        /** The game's id in its store, or null; set by the library reader beside {@link #storeSource}. */
        public String storeId;
        /** False for a game the host found in a store but has not added; it has no app id to launch. */
        public final boolean added;
        /** Seconds since the epoch it last ran on the host, or null when the host does not say. */
        public final Long lastPlayed;
        /** Minutes it was played, 0 for never, or null when the host does not say; set by the library reader. */
        public Integer playtimeMinutes;
        /** False when the host says it cannot start the game; null when it does not say. */
        public final Boolean launchable;
        /** The Steam collections or Playnite categories the host keeps it in, by name. */
        public final List<String> collections;
        /** Hidden on this device (the same list as the PC's app grid). */
        public final boolean hidden;
        public final String heroPath;
        public final String logoPath;
        public final String landscapePath;

        Source(String hostUuid, String hostName, int appId, String installId, String title, String gameId,
               boolean running, String coverPath, String coverVersion, Boolean installed, String appUuid,
               boolean hdrSupported) {
            this(hostUuid, hostName, appId, installId, title, gameId, running, coverPath, coverVersion, installed,
                    appUuid, hdrSupported, null, null, true, null, null);
        }

        Source(String hostUuid, String hostName, int appId, String installId, String title, String gameId,
               boolean running, String coverPath, String coverVersion, Boolean installed, String appUuid,
               boolean hdrSupported, String storeSource, String storeName, boolean added, Long lastPlayed,
               Boolean launchable) {
            this(hostUuid, hostName, appId, installId, title, gameId, running, coverPath, coverVersion, installed,
                    appUuid, hdrSupported, storeSource, storeName, added, lastPlayed, launchable,
                    Collections.emptyList(), false);
        }

        Source(String hostUuid, String hostName, int appId, String installId, String title, String gameId,
               boolean running, String coverPath, String coverVersion, Boolean installed, String appUuid,
               boolean hdrSupported, String storeSource, String storeName, boolean added, Long lastPlayed,
               Boolean launchable, List<String> collections, boolean hidden) {
            this(hostUuid, hostName, appId, installId, title, gameId, running, coverPath, coverVersion, installed,
                    appUuid, hdrSupported, storeSource, storeName, null, added, lastPlayed, null, launchable,
                    collections, hidden, null, null, null);
        }

        Source(String hostUuid, String hostName, int appId, String installId, String title, String gameId,
               boolean running, String coverPath, String coverVersion, Boolean installed, String appUuid,
               boolean hdrSupported, String storeSource, String storeName, String storeId, boolean added,
               Long lastPlayed, Integer playtimeMinutes, Boolean launchable, List<String> collections,
               boolean hidden, String heroPath, String logoPath, String landscapePath) {
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.installId = installId;
            this.title = title;
            this.gameId = gameId;
            this.running = running;
            this.coverPath = coverPath;
            this.coverVersion = coverVersion;
            this.installed = installed;
            this.appUuid = appUuid;
            this.hdrSupported = hdrSupported;
            this.storeSource = storeSource;
            this.storeName = storeName;
            this.storeId = storeId;
            this.added = added;
            this.lastPlayed = lastPlayed;
            this.playtimeMinutes = playtimeMinutes;
            this.launchable = launchable;
            this.collections = collections;
            this.hidden = hidden;
            this.heroPath = heroPath;
            this.logoPath = logoPath;
            this.landscapePath = landscapePath;
        }

        /**
         * The host's id of the game that survives a renumbering of its apps: the native install id, else the
         * classic app's UUID, else empty.
         */
        public String stableId() {
            if (installId != null && !installId.isEmpty()) {
                return installId;
            }
            return appUuid != null ? appUuid : "";
        }
    }

    /** A title, with every host that has it (hosts agree through the native API's game id). */
    public static final class Title {
        public final String key;
        public final String title;
        /** Running copies first, then in the order of the hosts. */
        public final List<Source> sources;

        Title(String key, String title, List<Source> sources) {
            this.key = key;
            this.title = title;
            this.sources = Collections.unmodifiableList(sources);
        }
    }

    /** One host's part of the snapshot. */
    public static final class Host {
        public final String uuid;
        public final String name;
        public final Status status;
        /** When the native library was read (LIVE: now); 0 for a classic list. */
        public final long fetchedAtMs;
        /** Why a live read failed, when the host shows CACHED or UNAVAILABLE for it; otherwise null. */
        public final NativeApiException error;
        /** The host's session, when it was read; null otherwise. */
        public final NativeSession session;
        public final List<Source> sources;
        /** The native API features the host advertised when its library was read; empty for a classic host. */
        public final Set<String> features;

        Host(String uuid, String name, Status status, long fetchedAtMs, NativeApiException error,
             NativeSession session, List<Source> sources) {
            this(uuid, name, status, fetchedAtMs, error, session, sources, Collections.emptySet());
        }

        Host(String uuid, String name, Status status, long fetchedAtMs, NativeApiException error,
             NativeSession session, List<Source> sources, Set<String> features) {
            this.uuid = uuid;
            this.name = name;
            this.status = status;
            this.fetchedAtMs = fetchedAtMs;
            this.error = error;
            this.session = session;
            this.sources = Collections.unmodifiableList(sources);
            this.features = Collections.unmodifiableSet(features);
        }

        public boolean has(String feature) {
            return features.contains(feature);
        }
    }

    public enum ContinueKind {
        /** Streaming or starting on the host now. */
        RUNNING,
        /** Suspended on the host; resuming picks it up. */
        SUSPENDED,
        /** Streamed from this device before. */
        RECENT
    }

    /** A game to come back to. */
    public static final class Continue {
        public final ContinueKind kind;
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        public final String title;
        /** The host's session for RUNNING and SUSPENDED entries of native hosts; otherwise null. */
        public final NativeSession session;
        /** The title's source on the host when its library lists it; otherwise null. */
        public final Source source;
        /** When this device last streamed it, 0 for none. */
        public final long playedAtMs;

        Continue(ContinueKind kind, String hostUuid, String hostName, int appId, String title,
                 NativeSession session, Source source, long playedAtMs) {
            this.kind = kind;
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.title = title;
            this.session = session;
            this.source = source;
            this.playedAtMs = playedAtMs;
        }
    }

    /** A game the player starred. */
    public static final class Starred {
        public final String hostUuid;
        public final String hostName;
        public final int appId;
        public final String title;
        /** The title's source on the host when its library lists it; otherwise null. */
        public final Source source;

        Starred(String hostUuid, String hostName, int appId, String title, Source source) {
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.title = title;
            this.source = source;
        }
    }

    /** Titles by name. */
    public final List<Title> titles;
    public final List<Host> hosts;
    /** Running first, then suspended, then recents, newest first. */
    public final List<Continue> continuePlaying;
    /** The starred games of the paired hosts, newest star first; a game the host's library dropped is left out. */
    public final List<Starred> favourites;

    ConsoleSnapshot(List<Title> titles, List<Host> hosts, List<Continue> continuePlaying, List<Starred> favourites) {
        this.titles = Collections.unmodifiableList(titles);
        this.hosts = Collections.unmodifiableList(hosts);
        this.continuePlaying = Collections.unmodifiableList(continuePlaying);
        this.favourites = Collections.unmodifiableList(favourites);
    }

    /** Nothing known: no hosts, no titles. */
    public static ConsoleSnapshot empty() {
        return new ConsoleSnapshot(new java.util.ArrayList<>(), new java.util.ArrayList<>(), new java.util.ArrayList<>(),
                new java.util.ArrayList<>());
    }

    public Host host(String uuid) {
        for (Host host : hosts) {
            if (host.uuid.equals(uuid)) {
                return host;
            }
        }
        return null;
    }

    /** Hydrates a [`ConsoleSnapshot`] from the JSON returned by `NightjarConsoleJni.repoCached`/`repoRefresh`. */
    public static ConsoleSnapshot fromRustJson(String raw, List<ComputerDetails> computers,
                                               Map<String, NativeApiException> errors) {
        if (raw == null || raw.isEmpty()) return empty();
        Json.Obj root = Json.parseObject(raw);
        if (root.has("error") && !root.has("titles")) return empty();

        Map<String, Boolean> hdrByHost = new HashMap<>();
        if (computers != null) {
            for (ComputerDetails c : computers) {
                if (c != null && c.uuid != null) {
                    hdrByHost.put(c.uuid, false);
                }
            }
        }

        Map<String, NativeSession> sessionsByHost = new HashMap<>();
        Json.Obj sessionsObj = root.optObject("sessions");
        if (sessionsObj != null) {
            java.util.Iterator<String> it = sessionsObj.keys();
            while (it.hasNext()) {
                String uuid = it.next();
                Json.Obj s = sessionsObj.optObject(uuid);
                if (s != null) sessionsByHost.put(uuid, NativeSession.parse(s));
            }
        }

        Map<String, List<Source>> sourcesByHost = new HashMap<>();
        Map<Source, Integer> sourceOrder = new java.util.IdentityHashMap<>();
        Map<String, Boolean> classicByHost = new HashMap<>();
        Json.Arr hostArr = root.optArray("host_states");
        for (int i = 0; hostArr != null && i < hostArr.length(); i++) {
            Json.Obj h = hostArr.getObject(i);
            String uuid = h.optString("uuid", "");
            classicByHost.put(uuid, "CLASSIC".equals(h.optString("status", "")));
        }

        Map<String, Title> titleByKey = new java.util.LinkedHashMap<>();
        parseEntryList(root.optArray("titles"), false, hdrByHost, classicByHost, sourcesByHost, sourceOrder, titleByKey);
        parseEntryList(root.optArray("hidden_titles"), true, hdrByHost, classicByHost, sourcesByHost, sourceOrder, titleByKey);
        List<Title> titles = new ArrayList<>(titleByKey.values());
        for (List<Source> list : sourcesByHost.values()) {
            list.sort(java.util.Comparator.comparingInt(s -> sourceOrder.getOrDefault(s, 0)));
        }

        List<Host> hosts = new ArrayList<>();
        Map<String, Host> hostById = new HashMap<>();
        for (int i = 0; hostArr != null && i < hostArr.length(); i++) {
            Json.Obj h = hostArr.getObject(i);
            String uuid = h.optString("uuid", "");
            Status status = Status.UNAVAILABLE;
            try { status = Status.valueOf(h.optString("status", "UNAVAILABLE")); } catch (IllegalArgumentException ignored) {}
            Set<String> features = new HashSet<>();
            Json.Arr featArr = h.optArray("features");
            for (int j = 0; featArr != null && j < featArr.length(); j++) {
                features.add(featArr.getString(j));
            }
            NativeApiException err = errors == null ? null : errors.get(uuid);
            if (err == null && h.has("error") && !h.isNull("error")) {
                NativeApiException.Kind kind = NativeApiException.Kind.of(h.optString("error_kind", "unavailable"));
                err = new NativeApiException(kind, h.optString("error", ""));
            }
            List<Source> hostSources = sourcesByHost.getOrDefault(uuid, Collections.emptyList());
            Host host = new Host(uuid, h.optString("name", ""), status, h.optLong("fetched_at_ms", 0L),
                    err, sessionsByHost.get(uuid), hostSources, features);
            hosts.add(host);
            hostById.put(uuid, host);
        }

        List<Continue> continuePlaying = new ArrayList<>();
        Set<String> playingKeys = new HashSet<>();
        Json.Arr contArr = root.optArray("continue_playing");
        for (int i = 0; contArr != null && i < contArr.length(); i++) {
            Json.Obj c = contArr.getObject(i);
            String hostUid = c.optString("host_uid", "");
            int appId = c.optInt("app_id", 0);
            String installId = c.optString("install_id", "");
            ContinueKind kind = "Suspended".equalsIgnoreCase(c.optString("playing", ""))
                    ? ContinueKind.SUSPENDED : ContinueKind.RUNNING;
            Source matched = findSource(sourcesByHost.get(hostUid), installId, appId);
            continuePlaying.add(new Continue(kind, hostUid, c.optString("host_name", ""), appId,
                    c.optString("title", ""), sessionsByHost.get(hostUid), matched, 0L));
            playingKeys.add(hostUid + ":" + appId);
        }
        Json.Arr recArr = root.optArray("recents");
        int recentsShown = 0;
        for (int i = 0; recArr != null && i < recArr.length(); i++) {
            Json.Obj r = recArr.getObject(i);
            String hostUid = r.optString("host", "");
            int appId = r.optInt("app_id", 0);
            String installId = r.optString("install_id", "");
            Host host = hostById.get(hostUid);
            if (host == null || playingKeys.contains(hostUid + ":" + appId)) continue;
            Source matched = findSource(sourcesByHost.get(hostUid), installId, appId);
            if (matched == null && host.status != Status.UNAVAILABLE) continue;
            if (matched != null && matched.hidden) continue;
            String title = matched != null ? matched.title : r.optString("title", "");
            long playedMs = r.optLong("at", 0L);
            continuePlaying.add(new Continue(ContinueKind.RECENT, hostUid, host.name, appId, title, null, matched, playedMs));
            if (++recentsShown == 10) break;
        }

        List<Starred> favourites = new ArrayList<>();
        Json.Arr favArr = root.optArray("starred");
        for (int i = 0; favArr != null && i < favArr.length(); i++) {
            Json.Obj f = favArr.getObject(i);
            String hostUid = f.optString("host", "");
            int appId = f.optInt("app_id", 0);
            String installId = f.optString("install_id", "");
            Host host = hostById.get(hostUid);
            if (host == null) continue;
            Source matched = findSource(sourcesByHost.get(hostUid), installId, appId);
            if (matched != null && !matched.added) matched = null;
            if (matched == null && host.status != Status.UNAVAILABLE) continue;
            int resolvedAppId = matched != null ? matched.appId : appId;
            String title = matched != null ? matched.title : f.optString("title", "");
            favourites.add(new Starred(hostUid, host.name, resolvedAppId, title, matched));
        }

        return new ConsoleSnapshot(titles, hosts, continuePlaying, favourites);
    }

    private static void parseEntryList(Json.Arr arr, boolean hidden, Map<String, Boolean> hdrByHost,
                                       Map<String, Boolean> classicByHost,
                                       Map<String, List<Source>> byHost,
                                       Map<Source, Integer> sourceOrder,
                                       Map<String, Title> outTitles) {
        for (int i = 0; arr != null && i < arr.length(); i++) {
            Json.Obj e = arr.getObject(i);
            String titleName = e.optString("title", "");
            Json.Arr copies = e.optArray("copies");
            List<Source> srcs = new ArrayList<>();
            for (int j = 0; copies != null && j < copies.length(); j++) {
                Json.Obj c = copies.getObject(j);
                String hostUid = c.optString("host_uid", "");
                boolean isClassic = Boolean.TRUE.equals(classicByHost.get(hostUid));
                List<String> cols = new ArrayList<>();
                Json.Arr colArr = c.optArray("collections");
                for (int k = 0; colArr != null && k < colArr.length(); k++) {
                    cols.add(colArr.getString(k));
                }
                Boolean installed = c.has("installed_raw") && !c.isNull("installed_raw")
                        ? c.optBoolean("installed_raw") : null;
                Long lp = c.has("last_played") && !c.isNull("last_played") ? c.optLong("last_played") : null;
                Integer pt = c.has("playtime_minutes") && !c.isNull("playtime_minutes")
                        ? c.optInt("playtime_minutes", 0) : null;
                Boolean launchable = c.has("launchable") && !c.isNull("launchable")
                        ? c.optBoolean("launchable") : null;
                String rawId = c.optString("install_id", "");
                String copyTitle = c.optString("title", titleName);
                Source s = new Source(
                        hostUid,
                        c.optString("host_name", ""),
                        c.optInt("app_id", 0),
                        isClassic ? null : rawId,
                        copyTitle.isEmpty() ? titleName : copyTitle,
                        c.isNull("game_id") ? null : c.optString("game_id", null),
                        c.optBoolean("running"),
                        c.isNull("cover_path") ? null : c.optString("cover_path", null),
                        c.optString("cover_version", ""),
                        installed,
                        isClassic ? rawId : null,
                        Boolean.TRUE.equals(hdrByHost.get(hostUid)),
                        c.isNull("store") ? null : c.optString("store", null),
                        c.isNull("store_name") ? null : c.optString("store_name", null),
                        c.isNull("store_id") ? null : c.optString("store_id", null),
                        c.optBoolean("added", true),
                        lp,
                        pt,
                        launchable,
                        Collections.unmodifiableList(cols),
                        hidden,
                        c.isNull("hero_path") ? null : c.optString("hero_path", null),
                        c.isNull("logo_path") ? null : c.optString("logo_path", null),
                        c.isNull("landscape_path") ? null : c.optString("landscape_path", null));
                srcs.add(s);
                sourceOrder.put(s, c.optInt("index", 0));
                byHost.computeIfAbsent(hostUid, key -> new ArrayList<>()).add(s);
            }
            if (!srcs.isEmpty()) {
                Source first = srcs.get(0);
                String rustKey = e.optString("key", "");
                String titleKey = first.gameId != null && !first.gameId.isEmpty()
                        ? "game:" + first.gameId
                        : (first.appId == 0 && (first.installId == null || first.installId.isEmpty()) && !rustKey.isEmpty())
                        ? rustKey
                        : "host:" + first.hostUuid + ":" + first.appId;
                Title existing = outTitles.get(titleKey);
                if (existing == null) {
                    outTitles.put(titleKey, new Title(titleKey, titleName, srcs));
                } else {
                    List<Source> combined = new ArrayList<>(existing.sources.size() + srcs.size());
                    combined.addAll(existing.sources);
                    combined.addAll(srcs);
                    outTitles.put(titleKey, new Title(existing.key, existing.title, combined));
                }
            }
        }
    }

    private static Source findSource(List<Source> list, String installId, int appId) {
        if (list == null) return null;
        for (Source s : list) {
            if (installId != null && !installId.isEmpty() && installId.equalsIgnoreCase(s.installId)) {
                return s;
            }
            if (appId > 0 && s.appId == appId) return s;
        }
        return null;
    }
}
