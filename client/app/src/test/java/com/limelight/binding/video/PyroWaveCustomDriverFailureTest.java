package com.limelight.binding.video;

import com.limelight.R;

import org.junit.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import java.util.stream.Stream;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

// nightjar#116: a custom driver that falls back to the system driver says why, on screen and in the log.
public class PyroWaveCustomDriverFailureTest {
    private static final String[] NOTICES = {"open", "load", "start", "no_gpu", "features", "stream"};

    @Test
    public void eachStageHasItsOwnNotice() {
        assertEquals(R.string.gpu_driver_failed_open,
                PyroWaveDecoderRenderer.customDriverFailureSummary(PyroWaveDecoderRenderer.DRIVER_NOT_OPENED));
        assertEquals(R.string.gpu_driver_failed_load,
                PyroWaveDecoderRenderer.customDriverFailureSummary(PyroWaveDecoderRenderer.DRIVER_NOT_LOADED));
        assertEquals(R.string.gpu_driver_failed_start,
                PyroWaveDecoderRenderer.customDriverFailureSummary(PyroWaveDecoderRenderer.DRIVER_NOT_STARTED));
        assertEquals(R.string.gpu_driver_failed_no_gpu,
                PyroWaveDecoderRenderer.customDriverFailureSummary(PyroWaveDecoderRenderer.DRIVER_NO_GPU));
        assertEquals(R.string.gpu_driver_failed_stream,
                PyroWaveDecoderRenderer.customDriverFailureSummary(PyroWaveDecoderRenderer.DRIVER_STREAM_FAILED));
        // The device checks' codes 1-8: the driver runs but lacks what PyroWave needs.
        for (int reason = 1; reason <= 8; reason++) {
            assertEquals(R.string.gpu_driver_failed_features, PyroWaveDecoderRenderer.customDriverFailureSummary(reason));
        }
        // A failed check, a crash or an older stored value keeps the general notice.
        assertEquals(R.string.gpu_driver_system_fallback, PyroWaveDecoderRenderer.customDriverFailureSummary(9));
        assertEquals(R.string.gpu_driver_system_fallback, PyroWaveDecoderRenderer.customDriverFailureSummary(0));
    }

    @Test
    public void theLogNamesTheStageAndTheLinkersMessage() {
        String log = PyroWaveDecoderRenderer.customDriverFailureLog(PyroWaveDecoderRenderer.DRIVER_NOT_LOADED,
                "dlopen failed: library \"libfoo.so\" not found");
        assertTrue(log, log.startsWith("its library did not load ("));
        assertTrue(log, log.contains("libfoo.so"));
        assertEquals("Vulkan started on it but it listed no GPU",
                PyroWaveDecoderRenderer.customDriverFailureLog(PyroWaveDecoderRenderer.DRIVER_NO_GPU, null));
        assertEquals("a GPU check failed (reason 3)", PyroWaveDecoderRenderer.customDriverFailureLog(3, ""));
    }

    // The native codes are device_path.h's; the Java side names them.
    @Test
    public void javaCodesMatchTheNativeOnes() throws Exception {
        String header = Files.readString(Path.of("src/main/jni/pyrowave-renderer/device_path.h"));
        assertEquals(PyroWaveDecoderRenderer.DRIVER_NOT_OPENED, nativeCode(header, "PYRO_DRIVER_NOT_OPENED"));
        assertEquals(PyroWaveDecoderRenderer.DRIVER_NOT_LOADED, nativeCode(header, "PYRO_DRIVER_NOT_LOADED"));
        assertEquals(PyroWaveDecoderRenderer.DRIVER_NOT_STARTED, nativeCode(header, "PYRO_DRIVER_NOT_STARTED"));
        assertEquals(PyroWaveDecoderRenderer.DRIVER_NO_GPU, nativeCode(header, "PYRO_DRIVER_NO_GPU"));
    }

    @Test
    public void everyLanguageHasEveryNotice() throws Exception {
        List<Path> files;
        try (Stream<Path> dirs = Files.list(Path.of("src/main/res"))) {
            files = dirs.map(dir -> dir.resolve("strings_gpu_driver.xml")).filter(Files::exists).collect(Collectors.toList());
        }
        assertTrue(files.size() >= 15);
        for (Path file : files) {
            String text = Files.readString(file);
            for (String notice : NOTICES) {
                assertTrue(file + " " + notice, text.contains("<string name=\"gpu_driver_failed_" + notice + "\">"));
            }
        }
    }

    private static int nativeCode(String header, String name) {
        Matcher match = Pattern.compile("constexpr int " + name + " = (\\d+);").matcher(header);
        assertTrue(name, match.find());
        return Integer.parseInt(match.group(1));
    }
}
