package com.limelight.nvstream.http;

import java.util.HashMap;
import java.util.Map;

/**
 * An invite to play over the internet, {@code rubylight://invite#v=1&k=...}, as the confirmation reads it: the PC's
 * name, what the friend may do and when the link ends. The secret in it is read only by libnightjar_core
 * (rubylight-protocol invite_uri.rs), which opens the invite ({@code InternetPlay.redeem}); this reader checks the
 * same required parts so that a link it shows can be opened.
 */
public final class InviteLink {
    public static final String PREFIX = "rubylight://invite#";
    /** The same link as Windows' shell hands it over, with the empty path's slash. */
    private static final String SHELL_PREFIX = "rubylight://invite/#";
    private static final int MAX_LEN = 4096;

    public static final String VIEW = "view";
    public static final String CONTROLLER = "controller";
    public static final String KEYBOARD_MOUSE = "controller-keyboard-mouse";

    /** The link as it came, handed to the engine to open. */
    public final String text;
    /** The PC's name, for display. */
    public final String name;
    /** Unix seconds the link ends. */
    public final long expires;
    /** One of {@link #VIEW}, {@link #CONTROLLER}, {@link #KEYBOARD_MOUSE}. */
    public final String permission;

    private InviteLink(String text, String name, long expires, String permission) {
        this.text = text;
        this.name = name;
        this.expires = expires;
        this.permission = permission;
    }

    public static boolean looksLikeLink(String text) {
        if (text == null) return false;
        String trimmed = text.trim();
        return trimmed.startsWith(PREFIX) || trimmed.startsWith(SHELL_PREFIX);
    }

    /** The invite, or null when the text is no invite this client reads. */
    public static InviteLink parse(String text) {
        if (!looksLikeLink(text)) return null;
        String trimmed = text.trim();
        if (trimmed.length() > MAX_LEN) return null;
        String fragment = trimmed.startsWith(PREFIX)
                ? trimmed.substring(PREFIX.length()) : trimmed.substring(SHELL_PREFIX.length());
        Map<String, String> pairs = new HashMap<>();
        for (String part : fragment.split("&", -1)) {
            int equals = part.indexOf('=');
            if (equals < 0) return null;
            String key = part.substring(0, equals);
            String value = PairingLink.decode(part.substring(equals + 1));
            if (value == null || pairs.containsKey(key)) return null;
            pairs.put(key, value);
        }
        if (!"1".equals(pairs.get("v"))) return null;
        if (PairingLink.fingerprint(orEmpty(pairs.get("k"))) == null) return null;
        if (PairingLink.fingerprint(orEmpty(pairs.get("fp"))) == null) return null;
        String id = orEmpty(pairs.get("id"));
        if (id.isEmpty() || id.length() > 64 || !id.matches("[A-Za-z0-9_-]+")) return null;
        String permission = orEmpty(pairs.get("perm"));
        if (!VIEW.equals(permission) && !CONTROLLER.equals(permission) && !KEYBOARD_MOUSE.equals(permission)) {
            return null;
        }
        String expires = orEmpty(pairs.get("exp"));
        if (!expires.matches("[0-9]{1,19}")) return null;
        String[] ports = orEmpty(pairs.get("ports")).split(",", -1);
        if (ports.length != 7) return null;
        for (String port : ports) {
            if (!port.matches("[0-9]{1,5}") || Integer.parseInt(port) > 65535) return null;
        }
        if (Integer.parseInt(ports[0]) == 0 || Integer.parseInt(ports[1]) == 0) return null;
        long end;
        try {
            end = Long.parseLong(expires);
        } catch (NumberFormatException e) {
            return null;
        }
        return new InviteLink(trimmed, orEmpty(pairs.get("name")), end, permission);
    }

    private static String orEmpty(String value) {
        return value == null ? "" : value;
    }

    /** The PC's name for a confirmation, cleaned as a pairing link's is. */
    public String shownName() {
        return PairingLink.shown(name);
    }

    /** Whether the link's own end has passed at {@code nowSeconds}; the PC decides, this spares a lookup. */
    public boolean expired(long nowSeconds) {
        return nowSeconds >= expires;
    }
}
