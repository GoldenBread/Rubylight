/**
 * Minimal pure-JDK JSON parser and builder for {@code NightjarJni} wrappers so JVM unit tests run
 * without Android's stubbed {@code org.json} classes.
 */
package com.limelight.nvstream.jni;

import java.util.ArrayList;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public final class Json {
    private Json() {}

    public static final class SyntaxException extends RuntimeException {
        public SyntaxException(String message) { super(message); }
    }

    public static Obj obj() { return new Obj(); }
    public static Arr arr() { return new Arr(); }

    public static Obj parseObject(String raw) {
        if (raw == null) throw new SyntaxException("Null JSON");
        Object v = new Parser(raw).parseRoot();
        if (!(v instanceof Obj)) throw new SyntaxException("Expected JSON object");
        return (Obj) v;
    }

    public static Arr parseArray(String raw) {
        if (raw == null) throw new SyntaxException("Null JSON");
        Object v = new Parser(raw).parseRoot();
        if (!(v instanceof Arr)) throw new SyntaxException("Expected JSON array");
        return (Arr) v;
    }

    public static final class Obj {
        private final LinkedHashMap<String, Object> map = new LinkedHashMap<>();

        public Obj put(String key, Object val) { map.put(key, val); return this; }
        public boolean has(String key) { return map.containsKey(key); }
        public boolean isNull(String key) { return map.get(key) == null; }
        public Iterator<String> keys() { return map.keySet().iterator(); }

        private Object req(String key) {
            if (!map.containsKey(key)) throw new SyntaxException("Missing key: " + key);
            Object v = map.get(key);
            if (v == null) throw new SyntaxException("Null value for key: " + key);
            return v;
        }

        public String getString(String key) {
            Object v = req(key);
            if (v instanceof String) return (String) v;
            return String.valueOf(v);
        }

        public String optString(String key) { return optString(key, ""); }
        public String optString(String key, String fallback) {
            Object v = map.get(key);
            return v == null ? fallback : (v instanceof String ? (String) v : String.valueOf(v));
        }

        public int getInt(String key) { return asNumber(req(key)).intValue(); }
        public int optInt(String key, int fallback) {
            Object v = map.get(key);
            return v instanceof Number ? ((Number) v).intValue() : fallback;
        }

        public long getLong(String key) { return asNumber(req(key)).longValue(); }
        public long optLong(String key) { return optLong(key, 0L); }
        public long optLong(String key, long fallback) {
            Object v = map.get(key);
            return v instanceof Number ? ((Number) v).longValue() : fallback;
        }

        public double getDouble(String key) { return asNumber(req(key)).doubleValue(); }
        public double optDouble(String key) { return optDouble(key, Double.NaN); }
        public double optDouble(String key, double fallback) {
            Object v = map.get(key);
            return v instanceof Number ? ((Number) v).doubleValue() : fallback;
        }

        public boolean getBoolean(String key) {
            Object v = req(key);
            if (v instanceof Boolean) return (Boolean) v;
            throw new SyntaxException("Expected boolean for " + key);
        }

        public boolean optBoolean(String key) { return optBoolean(key, false); }
        public boolean optBoolean(String key, boolean fallback) {
            Object v = map.get(key);
            return v instanceof Boolean ? (Boolean) v : fallback;
        }

        public Obj getObject(String key) {
            Object v = req(key);
            if (!(v instanceof Obj)) throw new SyntaxException("Expected object for " + key);
            return (Obj) v;
        }

        public Obj optObject(String key) {
            Object v = map.get(key);
            return v instanceof Obj ? (Obj) v : null;
        }

        public Arr getArray(String key) {
            Object v = req(key);
            if (!(v instanceof Arr)) throw new SyntaxException("Expected array for " + key);
            return (Arr) v;
        }

        @Override public String toString() {
            StringBuilder sb = new StringBuilder().append('{');
            boolean first = true;
            for (Map.Entry<String, Object> e : map.entrySet()) {
                if (!first) sb.append(',');
                first = false;
                writeString(sb, e.getKey());
                sb.append(':');
                writeValue(sb, e.getValue());
            }
            return sb.append('}').toString();
        }
    }

    public static final class Arr {
        private final List<Object> items = new ArrayList<>();

        public Arr add(Object val) { items.add(val); return this; }
        public int length() { return items.size(); }

        public String getString(int i) {
            Object v = items.get(i);
            if (v == null) throw new SyntaxException("Null item at " + i);
            return v instanceof String ? (String) v : String.valueOf(v);
        }

        public int getInt(int i) { return asNumber(items.get(i)).intValue(); }
        public long getLong(int i) { return asNumber(items.get(i)).longValue(); }
        public Obj getObject(int i) {
            Object v = items.get(i);
            if (!(v instanceof Obj)) throw new SyntaxException("Expected object at " + i);
            return (Obj) v;
        }

        @Override public String toString() {
            StringBuilder sb = new StringBuilder().append('[');
            for (int i = 0; i < items.size(); i++) {
                if (i > 0) sb.append(',');
                writeValue(sb, items.get(i));
            }
            return sb.append(']').toString();
        }
    }

    private static Number asNumber(Object v) {
        if (v instanceof Number) return (Number) v;
        if (v instanceof String) {
            try { return Double.valueOf((String) v); }
            catch (NumberFormatException e) { throw new SyntaxException("Not a number: " + v); }
        }
        throw new SyntaxException("Expected number, got " + v);
    }

    private static void writeValue(StringBuilder sb, Object v) {
        if (v == null) { sb.append("null"); }
        else if (v instanceof Boolean || v instanceof Number || v instanceof Obj || v instanceof Arr) {
            sb.append(v);
        } else {
            writeString(sb, String.valueOf(v));
        }
    }

    private static void writeString(StringBuilder sb, String s) {
        sb.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\b': sb.append("\\b"); break;
                case '\f': sb.append("\\f"); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20) sb.append(String.format("\\u%04x", (int) c));
                    else sb.append(c);
            }
        }
        sb.append('"');
    }

    private static final class Parser {
        private final String s;
        private int p;

        Parser(String s) { this.s = s; }

        Object parseRoot() {
            Object v = value();
            ws();
            if (p != s.length()) throw new SyntaxException("Trailing JSON content");
            return v;
        }

        private void ws() {
            while (p < s.length()) {
                char c = s.charAt(p);
                if (c == ' ' || c == '\n' || c == '\r' || c == '\t') p++;
                else break;
            }
        }

        private Object value() {
            ws();
            if (p >= s.length()) throw new SyntaxException("Unexpected EOF");
            char c = s.charAt(p);
            if (c == '{') return obj();
            if (c == '[') return arr();
            if (c == '"') return str();
            if (c == 't' && s.startsWith("true", p)) { p += 4; return Boolean.TRUE; }
            if (c == 'f' && s.startsWith("false", p)) { p += 5; return Boolean.FALSE; }
            if (c == 'n' && s.startsWith("null", p)) { p += 4; return null; }
            if (c == '-' || (c >= '0' && c <= '9')) return num();
            throw new SyntaxException("Unexpected char: " + c);
        }

        private Obj obj() {
            p++;
            Obj o = new Obj();
            ws();
            if (p < s.length() && s.charAt(p) == '}') { p++; return o; }
            while (true) {
                ws();
                if (p >= s.length() || s.charAt(p) != '"') throw new SyntaxException("Expected key");
                String key = str();
                ws();
                if (p >= s.length() || s.charAt(p) != ':') throw new SyntaxException("Expected ':'");
                p++;
                o.put(key, value());
                ws();
                if (p >= s.length()) throw new SyntaxException("Unterminated object");
                char next = s.charAt(p++);
                if (next == '}') return o;
                if (next != ',') throw new SyntaxException("Expected ',' or '}'");
            }
        }

        private Arr arr() {
            p++;
            Arr a = new Arr();
            ws();
            if (p < s.length() && s.charAt(p) == ']') { p++; return a; }
            while (true) {
                a.add(value());
                ws();
                if (p >= s.length()) throw new SyntaxException("Unterminated array");
                char next = s.charAt(p++);
                if (next == ']') return a;
                if (next != ',') throw new SyntaxException("Expected ',' or ']'");
            }
        }

        private String str() {
            p++;
            StringBuilder sb = new StringBuilder();
            while (p < s.length()) {
                char c = s.charAt(p++);
                if (c == '"') return sb.toString();
                if (c == '\\') {
                    if (p >= s.length()) throw new SyntaxException("Bad escape");
                    char e = s.charAt(p++);
                    switch (e) {
                        case '"': case '\\': case '/': sb.append(e); break;
                        case 'b': sb.append('\b'); break;
                        case 'f': sb.append('\f'); break;
                        case 'n': sb.append('\n'); break;
                        case 'r': sb.append('\r'); break;
                        case 't': sb.append('\t'); break;
                        case 'u':
                            if (p + 4 > s.length()) throw new SyntaxException("Bad unicode escape");
                            sb.append((char) Integer.parseInt(s.substring(p, p + 4), 16));
                            p += 4;
                            break;
                        default: throw new SyntaxException("Unknown escape: " + e);
                    }
                } else {
                    sb.append(c);
                }
            }
            throw new SyntaxException("Unterminated string");
        }

        private Number num() {
            int start = p;
            boolean fp = false;
            if (s.charAt(p) == '-') p++;
            while (p < s.length()) {
                char c = s.charAt(p);
                if (c >= '0' && c <= '9') { p++; }
                else if (c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') { fp = true; p++; }
                else break;
            }
            String raw = s.substring(start, p);
            try {
                if (!fp) return Long.parseLong(raw);
                return Double.parseDouble(raw);
            } catch (NumberFormatException e) {
                throw new SyntaxException("Bad number: " + raw);
            }
        }
    }
}
