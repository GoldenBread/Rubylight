package com.limelight.update;

import com.limelight.nvstream.jni.NightjarPolicyJni;

/** Reads sha256sum output ("hex  name" or "hex *name") and formats digests. */
public final class ChecksumFile {
    private ChecksumFile() {
    }

    /** The lowercase hex digest listed for the file name, or null when no line names it. */
    public static String find(String sums, String fileName) {
        if (sums == null || fileName == null) {
            return null;
        }
        return NightjarPolicyJni.checksumFileFind(sums, fileName);
    }

    public static String hex(byte[] bytes) {
        return NightjarPolicyJni.checksumFileHex(bytes);
    }
}
