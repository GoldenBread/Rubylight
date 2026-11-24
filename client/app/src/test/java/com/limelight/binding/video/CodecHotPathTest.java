package com.limelight.binding.video;

import com.limelight.nvstream.av.video.HostTimingReport;

import org.junit.Test;

import java.io.File;
import java.io.StringWriter;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.*;

/**
 * Unit tests for Slice 7 Android video hot-path optimizations: {@link IntRing},
 * zero-duplicate {@link HdrBitstream}, zero-alloc unchanged {@link Hdr10PlusSei},
 * O(1) {@link FrameLatencyStats} lookups and batched PyroWave events, and
 * zero-alloc {@link FramePacingPolicy#heldReportGrid}.
 */
public class CodecHotPathTest {

    @Test
    public void intRingPreservesFifoOrderAcrossWrapAndGrowth() {
        IntRing ring = new IntRing(4);
        assertTrue(ring.isEmpty());
        assertEquals(IntRing.EMPTY, ring.poll());
        for (int i = 0; i < 3; i++) ring.add(i);
        assertEquals(0, ring.poll());
        assertEquals(1, ring.poll());
        // Wrap around and force two growths.
        for (int i = 3; i < 20; i++) ring.add(i);
        assertEquals(18, ring.size());
        for (int i = 2; i < 20; i++) assertEquals(i, ring.poll());
        assertTrue(ring.isEmpty());
        assertEquals(IntRing.EMPTY, ring.poll());
        ring.add(42);
        ring.clear();
        assertTrue(ring.isEmpty());
        assertEquals(IntRing.EMPTY, ring.poll());
    }

    @Test
    public void hdrBitstreamPutPreservesDirectPositionAndLimitWithoutDuplicate() {
        ByteBuffer src = ByteBuffer.allocateDirect(16);
        for (int i = 0; i < 16; i++) src.put(i, (byte) (i + 1));
        src.position(3);
        src.limit(14);

        ByteBuffer dst = ByteBuffer.allocate(16);
        HdrBitstream.put(dst, src, 10, null);
        assertEquals(3, src.position());
        assertEquals(14, src.limit());
        assertEquals(10, dst.position());
        for (int i = 0; i < 10; i++) assertEquals((byte) (i + 1), dst.get(i));

        dst.clear();
        HdrBitstream.put(dst, src, 10, new int[] {3, 6});
        assertEquals(3, src.position());
        assertEquals(14, src.limit());
        assertEquals(7, dst.position());
        assertArrayEquals(new byte[] {1, 2, 3, 7, 8, 9, 10}, Arrays.copyOf(dst.array(), 7));
    }

    @Test
    public void hdrBitstreamByteArrayFastPathsMatchByteBuffer() {
        byte[] padded = {0x00, 0x00, 0x01, 0x26, 0x01, 0x00, 0x00};
        assertEquals(7, HdrBitstream.queuedLength(false, padded, 7));
        assertEquals(5, HdrBitstream.queuedLength(true, padded, 7));

        // HEVC prefix SEI (NAL type 39 -> (39 << 1) = 0x4E) carrying HDR Vivid T.35 header.
        byte[] vivid = {
                0x00, 0x00, 0x00, 0x01, 0x4E, 0x01,
                0x04, 0x06, 0x26, 0x00, 0x04, 0x00, 0x05, 0x11, (byte) 0x80,
                0x00, 0x00, 0x01, 0x02, 0x01
        };
        int[] rangeArr = new int[2];
        int[] rangeBuf = new int[2];
        assertTrue(HdrBitstream.findVividSei(vivid, vivid.length, rangeArr));
        assertTrue(HdrBitstream.findVividSei(ByteBuffer.wrap(vivid), vivid.length, rangeBuf));
        assertArrayEquals(rangeBuf, rangeArr);
    }

    @Test
    public void hdr10PlusIfChangedReturnsSameReferenceWhenMetadataMatches() {
        // HEVC prefix SEI with HDR10+ T.35 payload (B5 00 3C 00 01 04 55 66).
        byte[] hevc1 = {
                0x00, 0x00, 0x01, 0x4E, 0x01,
                0x04, 0x08, (byte) 0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x55, 0x66, (byte) 0x80,
                0x00, 0x00, 0x01, 0x02, 0x01
        };
        byte[] hevc2 = hevc1.clone();
        hevc2[14] = 0x77;

        Hdr10PlusSei.Scratch scratch = new Hdr10PlusSei.Scratch();
        byte[] first = Hdr10PlusSei.hevcIfChanged(hevc1, hevc1.length, null, scratch);
        assertNotNull(first);
        assertSame(first, Hdr10PlusSei.hevcIfChanged(hevc1, hevc1.length, first, scratch));
        assertSame(first, Hdr10PlusSei.hevcIfChanged(ByteBuffer.wrap(hevc1), hevc1.length, first, scratch));

        byte[] second = Hdr10PlusSei.hevcIfChanged(hevc2, hevc2.length, first, scratch);
        assertNotNull(second);
        assertNotSame(first, second);
        assertEquals(0x77, second[7]);

        // AV1 metadata OBU (type 5 -> (5 << 3) | 2 = 0x2A), size 10, type 4 (T.35), payload + 0x80.
        byte[] av1 = {
                0x2A, 0x0A, 0x04, (byte) 0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x55, 0x66, (byte) 0x80
        };
        byte[] av1First = Hdr10PlusSei.av1IfChanged(av1, av1.length, null, scratch);
        assertArrayEquals(first, av1First);
        assertSame(av1First, Hdr10PlusSei.av1IfChanged(av1, av1.length, av1First, scratch));
        assertSame(av1First, Hdr10PlusSei.av1IfChanged(ByteBuffer.wrap(av1), av1.length, av1First, scratch));
    }

    @Test
    public void frameLatencyStatsByNumberAndBatchedPyroWaveEventsWorkEndToEnd() throws Exception {
        FrameLatencyStats stats = new FrameLatencyStats();
        stats.onPyroWaveInput(10, 1000L, 1_000_000L, 1_100_000L, 1_200_000L, (char) 5, 50_000L, 1_250_000L);
        stats.onPyroWaveInput(11, 2000L, 2_000_000L, 2_100_000L, 2_200_000L, (char) 5, 66_000L, 2_250_000L);

        // Deliver batched events: frame 10 decoded -> queued (shown, timing) -> rendered;
        // frame 11 decoded -> queued -> surface_changed (timeNs == 0).
        long[] events = {
                FrameLatencyStats.PYROWAVE_EVENT_DECODED, 1000L, 2_000_000L, 0L,
                FrameLatencyStats.PYROWAVE_EVENT_QUEUED, 1000L, 2_100_000L, 3L,
                FrameLatencyStats.PYROWAVE_EVENT_RENDERED, 1000L, 2_500_000L, 0L,
                FrameLatencyStats.PYROWAVE_EVENT_DECODED, 2000L, 3_000_000L, 0L,
                FrameLatencyStats.PYROWAVE_EVENT_QUEUED, 2000L, 3_100_000L, 3L,
                FrameLatencyStats.PYROWAVE_EVENT_RENDERED, 2000L, 0L, 0L,
        };
        stats.applyPyroWaveEvents(events, 6, 4_000_000L);

        // Attach host timing to completed frame 10 via O(1) byNumber table before CSV flush.
        java.nio.ByteBuffer rawReport = java.nio.ByteBuffer.allocate(24).order(java.nio.ByteOrder.LITTLE_ENDIAN);
        rawReport.put((byte) HostTimingReport.VERSION).put((byte) 1).put((byte) 0).put((byte) 0);
        rawReport.putInt(10).putInt(1000).putInt(100).putInt(600).putInt(300);
        HostTimingReport report = HostTimingReport.parse(rawReport.array());
        assertNotNull(report);
        stats.onHostTiming(report, 5_000_000L);

        StringWriter csv = new StringWriter();
        long written = stats.writeCsv(csv);
        assertTrue(written > 0);
        String text = csv.toString();
        assertTrue(text.contains("10,1000,"));
        assertTrue(text.contains(",rendered,"));
        assertTrue(text.contains(",1.000000,0.100000,0.600000,0.300000,"));
        assertTrue(text.contains("11,2000,"));
        assertTrue(text.contains(",surface_changed,"));
    }

    @Test
    public void heldReportGridOutArrayMatchesAllocatingOverload() {
        long[] out = new long[2];
        long[] expected1 = FramePacingPolicy.heldReportGrid(100_000_000L, 8_333_333L, 200_000_000L, 2);
        FramePacingPolicy.heldReportGrid(100_000_000L, 8_333_333L, 200_000_000L, 2, out);
        assertArrayEquals(expected1, out);

        long[] expected2 = FramePacingPolicy.heldReportGrid(100_000_000L, 16_666_667L, 0L, 1);
        FramePacingPolicy.heldReportGrid(100_000_000L, 16_666_667L, 0L, 1, out);
        assertArrayEquals(expected2, out);
    }

    @Test
    public void pyroWaveDisplayClockRingIsolatesPublishedSnapshots() {
        PyroWaveDisplayClock clock = new PyroWaveDisplayClock();
        clock.reset(10_000_000L);
        long t = 1_000_000_000L;
        for (int i = 0; i < 4; i++) clock.observe(t + i * 10_000_000L);
        PyroWaveDisplayClock.Snapshot s0 = clock.snapshot();
        clock.observe(t + 40_000_000L);
        PyroWaveDisplayClock.Snapshot s1 = clock.snapshot();
        clock.observe(t + 50_000_000L);
        PyroWaveDisplayClock.Snapshot s2 = clock.snapshot();
        assertNotSame(s0, s1);
        assertNotSame(s1, s2);
        assertEquals(t + 30_000_000L, s0.vsyncNs);
        assertEquals(t + 40_000_000L, s1.vsyncNs);
        assertEquals(t + 50_000_000L, s2.vsyncNs);
        List<Long> intervals = Arrays.asList(s0.intervalNs, s1.intervalNs, s2.intervalNs);
        assertEquals(Arrays.asList(10_000_000L, 10_000_000L, 10_000_000L), intervals);
    }

    private static File findFile(String... candidates) {
        for (String path : candidates) {
            File file = new File(path);
            if (file.isFile()) return file;
        }
        throw new AssertionError("File not found in " + new File(".").getAbsolutePath()
                + ": " + Arrays.toString(candidates));
    }

    private static String jniDescriptor(Class<?> type) {
        if (type == void.class) return "V";
        if (type == int.class) return "I";
        if (type == long.class) return "J";
        if (type == short.class) return "S";
        if (type == byte.class) return "B";
        if (type == char.class) return "C";
        if (type == boolean.class) return "Z";
        if (type == float.class) return "F";
        if (type == double.class) return "D";
        if (type.isArray()) return "[" + jniDescriptor(type.getComponentType());
        return "L" + type.getName().replace('.', '/') + ";";
    }

    private static String jniDescriptor(Method method) {
        StringBuilder sb = new StringBuilder("(");
        for (Class<?> param : method.getParameterTypes()) sb.append(jniDescriptor(param));
        return sb.append(')').append(jniDescriptor(method.getReturnType())).toString();
    }

    private static String proguardSignature(Method method) {
        StringBuilder sb = new StringBuilder("public static ")
                .append(method.getReturnType().getTypeName()).append(' ')
                .append(method.getName()).append('(');
        Class<?>[] params = method.getParameterTypes();
        for (int i = 0; i < params.length; i++) {
            if (i > 0) sb.append(", ");
            sb.append(params[i].getTypeName());
        }
        return sb.append(");").toString();
    }

    @Test
    public void rustFastCallbacksMatchMoonBridgeAndAreKeptByProguard() throws Exception {
        String bridgeRs = new String(Files.readAllBytes(findFile(
                "../protocol/android/src/bridge.rs", "protocol/android/src/bridge.rs").toPath()),
                StandardCharsets.UTF_8);
        String proguard = new String(Files.readAllBytes(findFile(
                "proguard-rules.pro", "app/proguard-rules.pro").toPath()), StandardCharsets.UTF_8);

        Matcher block = Pattern.compile(
                "-keepclassmembers\\s+class\\s+com\\.limelight\\.nvstream\\.jni\\.MoonBridge\\s*\\{(.*?)\\}",
                Pattern.DOTALL).matcher(proguard);
        assertTrue("MoonBridge -keepclassmembers block missing from proguard-rules.pro", block.find());
        String keptMoonBridge = block.group(1);

        Class<?> moonBridge = Class.forName("com.limelight.nvstream.jni.MoonBridge", false,
                CodecHotPathTest.class.getClassLoader());
        Map<String, Method> methods = new TreeMap<>();
        for (Method m : moonBridge.getDeclaredMethods()) {
            methods.put(m.getName(), m);
        }

        Matcher lookup = Pattern.compile("id\\(\"(\\w+)\",\\s*\"([^\"]+)\"\\)").matcher(bridgeRs);
        int found = 0;
        while (lookup.find()) {
            String name = lookup.group(1);
            if (name.startsWith("bridge")) {
                continue;
            }
            found++;
            String expectedDesc = lookup.group(2);
            Method method = methods.get(name);
            assertNotNull("MoonBridge missing fast callback " + name, method);
            assertTrue(name + " must be public static",
                    Modifier.isPublic(method.getModifiers()) && Modifier.isStatic(method.getModifiers()));
            assertEquals("JNI signature mismatch for MoonBridge." + name, expectedDesc, jniDescriptor(method));
            String expectedRule = proguardSignature(method);
            assertTrue("proguard-rules.pro missing keep rule: " + expectedRule,
                    keptMoonBridge.contains(expectedRule));
        }
        assertEquals("expected 4 fast callbacks in bridge.rs", 4, found);
    }
}
