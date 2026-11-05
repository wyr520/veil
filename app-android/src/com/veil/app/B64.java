package com.veil.app;

import java.nio.charset.StandardCharsets;

/** Base64 that tolerates URL-safe alphabets, stray whitespace and missing padding. */
public final class B64 {

    private B64() {}

    public static byte[] decodeTolerant(String s) {
        String cleaned = s.replaceAll("\\s", "").replace('-', '+').replace('_', '/');
        int pad = (4 - cleaned.length() % 4) % 4;
        StringBuilder sb = new StringBuilder(cleaned);
        for (int i = 0; i < pad; i++) sb.append('=');
        return android.util.Base64.decode(sb.toString(), android.util.Base64.DEFAULT);
    }

    public static String decodeToString(String s) {
        return new String(decodeTolerant(s), StandardCharsets.UTF_8);
    }
}
