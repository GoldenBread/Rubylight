package com.limelight.nvstream.jni;

/**
 * Which streaming engine the last stream used and, when the stream could not start, why. Shown in
 * the stream overlays and the problem report, so a timing CSV or a report never has to guess.
 * Kept apart from MoonBridge, whose static initializer loads a native library, so reports and
 * tests can read it without native code.
 */
public final class StreamEngine {
    private StreamEngine() {}

    private static final String ENGINE = "Rust (nightjar-core)";

    private static volatile String summary = "no stream started in this app process";

    /** Records the start of a stream: {@code refusal} is why the engine cannot start it, or null. */
    static void record(String refusal) {
        summary = describe(refusal);
    }

    /** "Rust (nightjar-core)", or why the engine could not start the last stream. */
    public static String summary() {
        return summary;
    }

    /** A short label for the overlay's first line: "Rust". */
    public static String label() {
        return "Rust";
    }

    static String describe(String refusal) {
        return refusal == null ? ENGINE : ENGINE + " could not start the stream: " + refusal;
    }
}
