package com.veil.app;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * One proxy endpoint, parsed from a share URI. Only what the UI needs to show
 * and to hand off to a tunnel client — no crypto lives here.
 *
 * The URI is parsed by hand instead of with {@link java.net.URI}: for an
 * unregistered scheme like {@code vless}, URI may classify the authority as
 * registry-based and return {@code null} from {@code getHost()}.
 */
public class Node {

    public String raw;
    public String protocol;   // VLESS / Trojan / VMess / Hysteria2
    public String name;
    public String host;
    public int port;
    public String transport = "tcp";
    public String sni = "";
    public String path = "";
    public String uuid = "";
    public String password = "";
    public Integer latency = null;

    public String endpoint() {
        return host + ":" + port;
    }

    /** Two-letter badge used by the list. */
    public String badge() {
        if ("VLESS".equals(protocol)) return "VL";
        if ("Trojan".equals(protocol)) return "TR";
        if ("VMess".equals(protocol)) return "VM";
        if ("Hysteria2".equals(protocol)) return "HY";
        return "?";
    }

    public static Node parse(String raw) {
        String s = raw == null ? "" : raw.trim();
        if (s.isEmpty()) return null;
        try {
            if (s.startsWith("vmess://")) return parseVmess(s);
            if (s.startsWith("vless://")) return parseShare(s, "VLESS");
            if (s.startsWith("trojan://")) return parseShare(s, "Trojan");
            if (s.startsWith("hysteria2://") || s.startsWith("hy2://")) {
                return parseShare(s, "Hysteria2");
            }
        } catch (Exception ignored) {
            // A malformed link must not take the whole list down.
        }
        return null;
    }

    private static Node parseShare(String raw, String protocol) {
        int schemeEnd = raw.indexOf("://");
        String rest = raw.substring(schemeEnd + 3);

        String fragment = null;
        int hash = rest.indexOf('#');
        if (hash >= 0) {
            fragment = rest.substring(hash + 1);
            rest = rest.substring(0, hash);
        }
        String query = null;
        int qm = rest.indexOf('?');
        if (qm >= 0) {
            query = rest.substring(qm + 1);
            rest = rest.substring(0, qm);
        }
        int slash = rest.indexOf('/');
        if (slash >= 0) rest = rest.substring(0, slash);

        String userInfo = null;
        int at = rest.lastIndexOf('@');
        if (at >= 0) {
            userInfo = rest.substring(0, at);
            rest = rest.substring(at + 1);
        }

        String host;
        int port = -1;
        if (rest.startsWith("[")) {                 // [2001:db8::1]:443
            int close = rest.indexOf(']');
            if (close < 0) return null;
            host = rest.substring(1, close);
            if (close + 1 < rest.length() && rest.charAt(close + 1) == ':') {
                port = intOr(rest.substring(close + 2), -1);
            }
        } else {
            int colon = rest.lastIndexOf(':');
            if (colon > 0) {
                host = rest.substring(0, colon);
                port = intOr(rest.substring(colon + 1), -1);
            } else {
                host = rest;
            }
        }
        if (host == null || host.isEmpty()) return null;
        if (port <= 0) port = 443;

        Node n = new Node();
        n.raw = raw;
        n.protocol = protocol;
        n.host = host;
        n.port = port;

        if (userInfo != null) {
            String decoded = percentDecode(userInfo);
            if ("VLESS".equals(protocol)) n.uuid = decoded;
            else n.password = decoded;
        }

        Map<String, String> q = query(query);
        String type = q.get("type");
        if (type != null && !type.isEmpty()) {
            n.transport = type;
        } else {
            n.transport = "Hysteria2".equals(protocol) ? "quic" : "tcp";
        }
        String sni = q.get("sni");
        if (sni == null) sni = q.get("peer");
        if (sni == null) sni = q.get("host");
        if (sni != null) n.sni = sni;
        String p = q.get("path");
        if (p != null) n.path = p;

        n.name = (fragment != null && !fragment.isEmpty()) ? percentDecode(fragment) : host;
        return n;
    }

    private static Node parseVmess(String raw) throws Exception {
        String payload = raw.substring("vmess://".length()).trim();
        JSONObject o = new JSONObject(B64.decodeToString(payload));
        Node n = new Node();
        n.raw = raw;
        n.protocol = "VMess";
        n.host = o.optString("add", "");
        n.port = intOr(o.optString("port", "0").trim(), 0);
        n.uuid = o.optString("id", "");
        String ps = o.optString("ps", "");
        n.name = ps.isEmpty() ? n.host : ps;
        String net = o.optString("net", "tcp");
        n.transport = net.isEmpty() ? "tcp" : net;
        n.sni = o.optString("sni", o.optString("host", ""));
        n.path = o.optString("path", "");
        if (n.host.isEmpty() || n.port <= 0) return null;
        return n;
    }

    static Map<String, String> query(String rawQuery) {
        Map<String, String> out = new LinkedHashMap<>();
        if (rawQuery == null || rawQuery.isEmpty()) return out;
        for (String pair : rawQuery.split("&")) {
            int i = pair.indexOf('=');
            if (i <= 0) continue;
            out.put(pair.substring(0, i), percentDecode(pair.substring(i + 1)));
        }
        return out;
    }

    private static int intOr(String s, int fallback) {
        try {
            return Integer.parseInt(s.trim());
        } catch (Exception e) {
            return fallback;
        }
    }

    /** RFC 3986 percent decoding. Unlike URLDecoder, '+' stays a plus. */
    static String percentDecode(String s) {
        if (s == null) return "";
        if (s.indexOf('%') < 0) return s;
        ByteArrayOutputStream out = new ByteArrayOutputStream(s.length());
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            if (c == '%' && i + 2 < s.length()) {
                int hi = Character.digit(s.charAt(i + 1), 16);
                int lo = Character.digit(s.charAt(i + 2), 16);
                if (hi >= 0 && lo >= 0) {
                    out.write((hi << 4) | lo);
                    i += 2;
                    continue;
                }
            }
            byte[] b = String.valueOf(c).getBytes(StandardCharsets.UTF_8);
            out.write(b, 0, b.length);
        }
        return new String(out.toByteArray(), StandardCharsets.UTF_8);
    }
}
