package com.limelight.nativeapi;

import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarConsoleJni;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Guests from other devices (GET /session/guests, feature session.guests): the devices that joined the game another
 * device plays, and the joins waiting for that device to let them in.
 */
public final class NativeGuests {
    public static final String VIEW = "view";
    public static final String CONTROLLER = "controller";
    public static final String KEYBOARD_MOUSE = "controller-keyboard-mouse";
    /** How often the device playing reads the list while it streams: the host waits 45 s for an answer. */
    public static final long POLL_MS = 2_000;

    /** Whether this device may answer joins and manage guests: the device playing, or one with Host control. */
    public final boolean manage;
    /** This device's own guest id, or -1 when it is no guest. */
    public final int you;
    /** The name of the device playing, or null. */
    public final String owner;
    public final String defaultPermission;
    public final int maxGuests;
    public final long version;
    /** The guest holding the keyboard and mouse, or -1 while the device playing holds them. */
    public final int keyboardMouse;
    public final List<Guest> guests;
    public final List<Request> waiting;

    public static final class Guest {
        public final int id;
        public final String name;
        public final String permission;
        public final boolean streaming;

        public Guest(int id, String name, String permission, boolean streaming) {
            this.id = id;
            this.name = name;
            this.permission = permission;
            this.streaming = streaming;
        }
    }

    public static final class Request {
        public final int id;
        public final String name;
        public final long secondsLeft;

        public Request(int id, String name, long secondsLeft) {
            this.id = id;
            this.name = name;
            this.secondsLeft = secondsLeft;
        }
    }

    public NativeGuests(boolean manage, int you, String owner, String defaultPermission, int maxGuests, long version,
            int keyboardMouse, List<Guest> guests, List<Request> waiting) {
        this.manage = manage;
        this.you = you;
        this.owner = owner;
        this.defaultPermission = defaultPermission;
        this.maxGuests = maxGuests;
        this.version = version;
        this.keyboardMouse = keyboardMouse;
        this.guests = Collections.unmodifiableList(guests);
        this.waiting = Collections.unmodifiableList(waiting);
    }

    static NativeGuests parse(Json.Obj json) {
        List<Guest> guests = new ArrayList<>();
        Json.Arr list = json.optArray("guests");
        for (int i = 0; list != null && i < list.length(); i++) {
            Json.Obj guest = list.getObject(i);
            guests.add(new Guest(guest.getInt("id"), guest.getString("name"), guest.getString("permission"),
                    guest.optBoolean("streaming")));
        }
        List<Request> waiting = new ArrayList<>();
        Json.Arr requests = json.optArray("waiting");
        for (int i = 0; requests != null && i < requests.length(); i++) {
            Json.Obj request = requests.getObject(i);
            waiting.add(new Request(request.getInt("id"), request.getString("name"), request.optLong("seconds_left")));
        }
        return new NativeGuests(json.optBoolean("manage"), optId(json, "you"),
                json.isNull("owner") ? null : json.optString("owner", null),
                json.optString("default_permission", CONTROLLER), json.optInt("max_guests", 0), json.optLong("version"),
                optId(json, "keyboard_mouse"), guests, waiting);
    }

    private static int optId(Json.Obj json, String key) {
        return json.has(key) && !json.isNull(key) ? json.optInt(key, -1) : -1;
    }

    /** Whether {@code permission} lets a guest take the keyboard and mouse. */
    public static boolean usesKeyboardMouse(String permission) {
        return KEYBOARD_MOUSE.equals(permission);
    }

    /**
     * Whether a stream that ended with the host's {@code reason} ended because the player or the game ended this
     * device's guest place: Nightjar then doesn't reconnect.
     */
    public static boolean ended(String reason) {
        return reason != null && NightjarConsoleJni.guestsEnded(reason);
    }

    /** An invite just made: the link to send (a secret) and when it ends, in Unix seconds. */
    public static final class Invite {
        public final String link;
        public final long expires;

        public Invite(String link, long expires) {
            this.link = link;
            this.expires = expires;
        }

        @Override
        public String toString() {
            return "Invite{expires=" + expires + "}";
        }
    }
}
