package com.limelight.nvstream.jni;

import org.junit.Test;

import static org.junit.Assert.*;

public class RustBridgeTest {
    private static final String SUNSHINE = "7.1.431.-1";

    @Test public void ordinaryStreamsStart() {
        assertNull(RustBridge.hostRefusal(SUNSHINE, 0x0101, false));
        assertNull(RustBridge.hostRefusal("7.1.450.0", 0x0001, false));
        // Input-only streams negotiate no codec.
        assertNull(RustBridge.hostRefusal(SUNSHINE, 0, true));
    }

    @Test public void unsupportedHostsAreRefused() {
        assertNotNull(RustBridge.hostRefusal("7.1.430.0", 0x0101, false));
        assertNotNull(RustBridge.hostRefusal("5.0.0", 0x0101, false));
        assertNotNull(RustBridge.hostRefusal(null, 0x0101, false));
        assertNotNull(RustBridge.hostRefusal("garbage", 0x0101, false));
        assertNotNull(RustBridge.hostRefusal(SUNSHINE, 0, false));
    }

    @Test public void everyRefusalNamesItsCauseAndWhatToDo() {
        String old = RustBridge.hostRefusal("7.1.430.0", 0x0101, false);
        assertTrue(old.contains("GFE 3.22"));
        assertTrue(old.contains("7.1.430.0"));
        assertTrue(old.contains("Update"));
        assertTrue(RustBridge.hostRefusal(SUNSHINE, 0, false).contains("codec"));
    }

    @Test public void theEngineSummaryNamesTheEngineOrWhyItCouldNotStart() {
        StreamEngine.record(null);
        assertEquals("Rust (nightjar-core)", StreamEngine.summary());
        assertEquals("Rust", StreamEngine.label());

        String refusal = RustBridge.hostRefusal("7.1.430.0", 0x0101, false);
        StreamEngine.record(refusal);
        assertEquals("Rust (nightjar-core) could not start the stream: " + refusal, StreamEngine.summary());
        StreamEngine.record(null);
    }

    @Test public void aMissingLibraryIsRecordedOnceAndRefusesTheStartInsteadOfThrowing() {
        assertTrue(RustBridge.isAvailable());
        assertNull(RustBridge.loadFailure());
        assertNull(RustBridge.startRefusal(SUNSHINE, 0x0101, false));

        NightjarJni.Loader loader = new NightjarJni.Loader();
        int[] attempts = {0};
        Runnable missing = () -> {
            attempts[0]++;
            throw new UnsatisfiedLinkError("missing");
        };
        assertFalse(loader.isAvailable(missing));
        assertFalse(loader.isAvailable(missing));
        assertEquals(1, attempts[0]);
        assertNotNull(loader.failure());
        assertTrue(loader.failure().contains("missing"));

        NightjarJni.forceUnavailableForTest(new UnsatisfiedLinkError("no library"));
        try {
            assertFalse(RustBridge.isAvailable());
            assertNotNull(RustBridge.loadFailure());
            String refusal = RustBridge.startRefusal(SUNSHINE, 0x0101, false);
            assertNotNull(refusal);
            assertTrue(refusal.contains("libnightjar_core.so"));
            assertTrue(refusal.contains(RustBridge.loadFailure()));
            // The host's reason comes first when both apply.
            assertTrue(RustBridge.startRefusal("7.1.430.0", 0x0101, false).contains("GFE 3.22"));
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
    }

    @Test public void appVersionsCompareNumerically() {
        int[] min = {7, 1, 431};
        assertTrue(RustBridge.appVersionAtLeast("7.1.431", min));
        assertTrue(RustBridge.appVersionAtLeast("7.2", min));
        assertTrue(RustBridge.appVersionAtLeast("8.0.0.0", min));
        assertFalse(RustBridge.appVersionAtLeast("7.1", min));
        assertFalse(RustBridge.appVersionAtLeast("7.1.-1", min));
    }

    @Test public void forceUnavailableDoesNotPoisonPolicyOrHttpStaticInit() throws Exception {
        ClassLoader parent = NightjarJni.class.getClassLoader();
        ClassLoader isolated = new ClassLoader(parent) {
            @Override
            protected Class<?> loadClass(String name, boolean resolve) throws ClassNotFoundException {
                if (NightjarPolicyJni.class.getName().equals(name)
                        || NightjarHttpJni.class.getName().equals(name)) {
                    Class<?> loaded = findLoadedClass(name);
                    if (loaded == null) {
                        String res = "/" + name.replace('.', '/') + ".class";
                        try (java.io.InputStream in = parent.getResourceAsStream(res.substring(1))) {
                            if (in == null) throw new ClassNotFoundException(name);
                            java.io.ByteArrayOutputStream buf = new java.io.ByteArrayOutputStream();
                            byte[] tmp = new byte[4096];
                            int n;
                            while ((n = in.read(tmp)) != -1) buf.write(tmp, 0, n);
                            byte[] bytes = buf.toByteArray();
                            loaded = defineClass(name, bytes, 0, bytes.length);
                        } catch (java.io.IOException e) {
                            throw new ClassNotFoundException(name, e);
                        }
                    }
                    if (resolve) resolveClass(loaded);
                    return loaded;
                }
                return super.loadClass(name, resolve);
            }
        };
        NightjarJni.forceUnavailableForTest(new UnsatisfiedLinkError("no library"));
        try {
            Class.forName(NightjarPolicyJni.class.getName(), true, isolated);
            Class.forName(NightjarHttpJni.class.getName(), true, isolated);
        } finally {
            NightjarJni.forceUnavailableForTest(null);
        }
        assertTrue(NightjarPolicyJni.abrBuiltInFloorKbps(1920, 1080, 60, 0) > 0);
        NightjarHttpJni.setSecondScreen(false);
    }
}
