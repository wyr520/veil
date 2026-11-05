package com.veil.app;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/** Fetches a subscription URL and turns it into share links. */
public final class Sub {

    private Sub() {}

    /**
     * @param input either an http(s) subscription URL, or pasted links
     * @return raw share URIs, one per line, in the order the server sent them
     */
    public static List<String> load(String input) throws Exception {
        String text = input.trim();
        if (text.startsWith("http://") || text.startsWith("https://")) {
            text = httpGet(text);
        }
        return split(text);
    }

    static List<String> split(String body) {
        String text = body;
        if (!text.contains("://")) {
            // Entire body is base64 (the usual subscription encoding).
            try {
                text = B64.decodeToString(text);
            } catch (Exception ignored) {
            }
        }
        List<String> out = new ArrayList<>();
        for (String line : text.split("[\\r\\n]+")) {
            String s = line.trim();
            if (s.contains("://") && Node.parse(s) != null) out.add(s);
        }
        return out;
    }

    private static String httpGet(String url) throws Exception {
        HttpURLConnection conn = null;
        try {
            conn = (HttpURLConnection) new URL(url).openConnection();
            conn.setConnectTimeout(12000);
            conn.setReadTimeout(20000);
            conn.setInstanceFollowRedirects(true);
            conn.setRequestProperty("User-Agent", "veil-app/0.1 (v2rayN compatible)");
            conn.setRequestProperty("Accept", "*/*");
            int code = conn.getResponseCode();
            if (code != 200) throw new IllegalStateException("HTTP " + code);
            InputStream in = conn.getInputStream();
            ByteArrayOutputStream buf = new ByteArrayOutputStream();
            byte[] tmp = new byte[8192];
            int n;
            while ((n = in.read(tmp)) > 0) buf.write(tmp, 0, n);
            in.close();
            return new String(buf.toByteArray(), StandardCharsets.UTF_8);
        } finally {
            if (conn != null) conn.disconnect();
        }
    }
}
