/**
 * Unit tests for {@link Json} strict RFC 8259 parsing and non-finite number rejection.
 */
package com.limelight.nvstream.jni;

import org.junit.Test;

import static org.junit.Assert.*;

public class JsonTest {
    @Test
    public void unicodeEscapesRequireFourStrictHexDigits() {
        assertEquals("Aä", Json.parseObject("{\"s\":\"\\u0041\\u00e4\"}").getString("s"));
        assertEquals("\u0000", Json.parseObject("{\"s\":\"\\u0000\"}").getString("s"));
        for (String bad : new String[] {
                "{\"s\":\"\\uZZZZ\"}",
                "{\"s\":\"\\u+041\"}",
                "{\"s\":\"\\u-001\"}",
                "{\"s\":\"\\u 041\"}",
                "{\"s\":\"\\u12\"}",
                "{\"s\":\"\\u123\"}",
        }) {
            assertThrows(bad, Json.SyntaxException.class, () -> Json.parseObject(bad));
        }
    }

    @Test
    public void nonFiniteNumbersAreRejectedOnWrite() {
        for (Number bad : new Number[] {
                Double.NaN, Double.POSITIVE_INFINITY, Double.NEGATIVE_INFINITY,
                Float.NaN, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY,
        }) {
            assertThrows(String.valueOf(bad), Json.SyntaxException.class,
                    () -> Json.obj().put("rtt_ms", bad).toString());
            assertThrows(String.valueOf(bad), Json.SyntaxException.class,
                    () -> Json.arr().add(bad).toString());
        }
        assertEquals("{\"rtt_ms\":12.5}", Json.obj().put("rtt_ms", 12.5).toString());
        assertEquals("[1.5]", Json.arr().add(1.5f).toString());
    }

    @Test
    public void leadingZeroesAndMalformedNumbersAreRejected() {
        assertEquals(0L, Json.parseObject("{\"n\":0}").getLong("n"));
        assertEquals(0L, Json.parseObject("{\"n\":-0}").getLong("n"));
        assertEquals(0.5, Json.parseObject("{\"n\":0.5}").getDouble("n"), 1e-9);
        assertEquals(-0.001, Json.parseObject("{\"n\":-1e-3}").getDouble("n"), 1e-9);
        for (String bad : new String[] {
                "{\"n\":01}",
                "{\"n\":-01}",
                "{\"n\":00.5}",
                "{\"n\":1.}",
                "{\"n\":-}",
                "{\"n\":1e}",
                "{\"n\":1e+}",
                "{\"n\":1+-2}",
                "{\"n\":1e1000}",
        }) {
            assertThrows(bad, Json.SyntaxException.class, () -> Json.parseObject(bad));
        }
    }
}
