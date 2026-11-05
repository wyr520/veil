package com.veil.app;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Bundle;
import android.text.TextUtils;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.InputMethodManager;
import android.widget.BaseAdapter;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/**
 * Single-screen client: import a subscription, list the nodes, inspect one and
 * hand the share link to a tunnel client. Everything is a framework view, so the
 * APK ends up with zero third-party dependencies.
 */
public class MainActivity extends Activity {

    private static final int BG = 0xFF0B0F14;
    private static final int CARD = 0xFF141A22;
    private static final int BORDER = 0xFF1F2630;
    private static final int TEXT = 0xFFFFFFFF;
    private static final int DIM = 0xFF6B7885;
    private static final int ACCENT = 0xFF3DDC97;

    private static final String PREFS = "veil";
    private static final String KEY_SOURCE = "source";
    private static final String KEY_LINKS = "links";

    private final List<Node> nodes = new ArrayList<>();
    private EditText input;
    private TextView status;
    private NodeAdapter adapter;
    private SharedPreferences prefs;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        setContentView(buildUi());
        input.setText(prefs.getString(KEY_SOURCE, ""));
        String saved = prefs.getString(KEY_LINKS, "");
        if (!TextUtils.isEmpty(saved)) {
            applyLinks(Sub.split(saved), false);
        }
    }

    // ------------------------------------------------------------------ UI

    private View buildUi() {
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(BG);
        root.setPadding(dp(16), dp(30), dp(16), 0);

        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);

        TextView logo = new TextView(this);
        logo.setText("V");
        logo.setTextColor(0xFF06121A);
        logo.setTextSize(16);
        logo.setTypeface(Typeface.DEFAULT_BOLD);
        logo.setGravity(Gravity.CENTER);
        GradientDrawable logoBg = new GradientDrawable(
                GradientDrawable.Orientation.TL_BR,
                new int[]{0xFF3DDC97, 0xFF2BB3FF});
        logoBg.setCornerRadius(dp(9));
        logo.setBackground(logoBg);
        header.addView(logo, new LinearLayout.LayoutParams(dp(30), dp(30)));

        TextView title = new TextView(this);
        title.setText("  Veil");
        title.setTextColor(TEXT);
        title.setTextSize(21);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        header.addView(title);

        TextView sub = new TextView(this);
        sub.setText("  多协议代理");
        sub.setTextColor(DIM);
        sub.setTextSize(11);
        header.addView(sub);

        root.addView(header);

        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setBackground(rounded(CARD, dp(16), BORDER));
        card.setPadding(dp(14), dp(14), dp(14), dp(12));
        LinearLayout.LayoutParams cardLp =
                new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT);
        cardLp.topMargin = dp(18);
        root.addView(card, cardLp);

        TextView label = new TextView(this);
        label.setText("订阅地址 / 分享链接");
        label.setTextColor(DIM);
        label.setTextSize(11);
        card.addView(label);

        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        LinearLayout.LayoutParams rowLp =
                new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT);
        rowLp.topMargin = dp(8);
        card.addView(row, rowLp);

        input = new EditText(this);
        input.setHint("https://node.example.com/sub/xxxx");
        input.setHintTextColor(0xFF4E5C69);
        input.setTextColor(TEXT);
        input.setTextSize(13);
        input.setSingleLine(true);
        input.setBackground(rounded(0xFF0F151C, dp(10), BORDER));
        input.setPadding(dp(10), dp(9), dp(10), dp(9));
        row.addView(input, new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f));

        Button go = new Button(this);
        go.setText("导入");
        go.setAllCaps(false);
        go.setTextColor(0xFF06121A);
        go.setTextSize(13);
        go.setTypeface(Typeface.DEFAULT_BOLD);
        go.setBackground(rounded(ACCENT, dp(10), 0));
        go.setPadding(dp(16), dp(8), dp(16), dp(8));
        go.setStateListAnimator(null);
        LinearLayout.LayoutParams goLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        goLp.leftMargin = dp(8);
        row.addView(go, goLp);
        go.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { doImport(); }
        });

        status = new TextView(this);
        status.setText("填订阅地址，或直接粘贴一条 vless:// 链接");
        status.setTextColor(DIM);
        status.setTextSize(11);
        LinearLayout.LayoutParams statusLp =
                new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT);
        statusLp.topMargin = dp(10);
        card.addView(status, statusLp);

        ListView list = new ListView(this);
        list.setDivider(null);
        list.setDividerHeight(dp(8));
        list.setPadding(0, dp(14), 0, dp(14));
        list.setClipToPadding(false);
        list.setSelector(new android.graphics.drawable.ColorDrawable(Color.TRANSPARENT));
        adapter = new NodeAdapter();
        list.setAdapter(adapter);
        list.setOnItemClickListener((parent, view, position, id) -> showDetail(nodes.get(position)));
        root.addView(list, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));

        TextView footer = new TextView(this);
        footer.setText("长按节点可单独测延迟 · 全局代理请把链接导入 v2rayNG / Hiddify");
        footer.setTextColor(0xFF3A4653);
        footer.setTextSize(10);
        footer.setGravity(Gravity.CENTER);
        footer.setPadding(0, 0, 0, dp(10));
        root.addView(footer);

        return root;
    }

    // -------------------------------------------------------------- import

    private void doImport() {
        final String source = input.getText().toString().trim();
        if (source.isEmpty()) {
            toast("请输入订阅地址或分享链接");
            return;
        }
        hideKeyboard();
        status.setTextColor(DIM);
        status.setText("正在拉取…");
        new Thread(() -> {
            try {
                final List<String> links = Sub.load(source);
                runOnUiThread(() -> {
                    prefs.edit().putString(KEY_SOURCE, source).apply();
                    applyLinks(links, true);
                });
            } catch (final Exception e) {
                runOnUiThread(() -> {
                    status.setTextColor(0xFFFF6B6B);
                    status.setText("拉取失败：" + e.getMessage());
                });
            }
        }).start();
    }

    private void applyLinks(List<String> links, boolean persist) {
        nodes.clear();
        for (String link : links) {
            Node n = Node.parse(link);
            if (n != null && !containsRaw(n.raw)) nodes.add(n);
        }
        adapter.notifyDataSetChanged();
        if (nodes.isEmpty()) {
            status.setTextColor(0xFFFFB020);
            status.setText("没有解析到节点，检查一下链接格式");
            return;
        }
        status.setTextColor(DIM);
        status.setText(nodes.size() + " 个节点 · 点一个查看详情");
        if (persist) {
            prefs.edit().putString(KEY_LINKS, TextUtils.join("\n", links)).apply();
        }
    }

    private boolean containsRaw(String raw) {
        for (Node n : nodes) {
            if (n.raw.equals(raw)) return true;
        }
        return false;
    }

    // -------------------------------------------------------------- detail

    private void showDetail(final Node n) {
        ScrollView scroll = new ScrollView(this);
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.setPadding(dp(20), dp(4), dp(20), 0);

        TextView meta = new TextView(this);
        meta.setText(metaText(n));
        meta.setTextColor(0xFFB9C6D2);
        meta.setTextSize(12);
        meta.setLineSpacing(dp(3), 1f);
        box.addView(meta);

        TextView link = new TextView(this);
        link.setText(n.raw);
        link.setTextColor(0xFF8FA6B4);
        link.setTextSize(10);
        link.setTypeface(Typeface.MONOSPACE);
        link.setTextIsSelectable(true);
        link.setBackground(rounded(0xFF0F151C, dp(10), BORDER));
        link.setPadding(dp(10), dp(10), dp(10), dp(10));
        LinearLayout.LayoutParams linkLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        linkLp.topMargin = dp(12);
        box.addView(link, linkLp);

        final TextView probeResult = new TextView(this);
        probeResult.setTextColor(DIM);
        probeResult.setTextSize(11);
        probeResult.setText(n.latency == null ? "" : "TCP 延迟 " + n.latency + " ms");
        LinearLayout.LayoutParams probeLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        probeLp.topMargin = dp(10);
        box.addView(probeResult, probeLp);

        LinearLayout row1 = new LinearLayout(this);
        row1.setOrientation(LinearLayout.HORIZONTAL);
        LinearLayout.LayoutParams row1Lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        row1Lp.topMargin = dp(14);
        box.addView(row1, row1Lp);
        row1.addView(action("复制链接", v -> {
            copy(n.raw);
            toast("已复制到剪贴板");
        }), weight(false));
        row1.addView(action("分享", v -> share(n.raw)), weight(true));

        LinearLayout row2 = new LinearLayout(this);
        row2.setOrientation(LinearLayout.HORIZONTAL);
        LinearLayout.LayoutParams row2Lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        row2Lp.topMargin = dp(8);
        box.addView(row2, row2Lp);
        row2.addView(action("测延迟", v -> {
            probeResult.setTextColor(DIM);
            probeResult.setText("测试中…");
            new Thread(() -> {
                final Integer ms = probe(n.host, n.port);
                runOnUiThread(() -> {
                    n.latency = ms;
                    adapter.notifyDataSetChanged();
                    probeResult.setTextColor(ms == null ? 0xFFFF6B6B : ACCENT);
                    probeResult.setText(ms == null
                            ? "连不上 " + n.endpoint()
                            : "TCP 延迟 " + ms + " ms");
                });
            }).start();
        }), weight(false));
        row2.addView(action("客户端打开", v -> openInClient(n.raw)), weight(true));

        scroll.addView(box);
        new AlertDialog.Builder(this)
                .setTitle(n.name)
                .setView(scroll)
                .setNegativeButton("关闭", null)
                .show();
    }

    private String metaText(Node n) {
        StringBuilder sb = new StringBuilder();
        sb.append(n.protocol).append(" · ").append(n.transport.toUpperCase(Locale.ROOT))
                .append(" · ").append(n.endpoint());
        if (!n.sni.isEmpty()) sb.append("\nSNI   ").append(n.sni);
        if (!n.path.isEmpty()) sb.append("\nPATH  ").append(n.path);
        if (!n.uuid.isEmpty()) sb.append("\nUUID  ").append(n.uuid);
        if (!n.password.isEmpty()) sb.append("\n密码  ").append(n.password);
        return sb.toString();
    }

    private Button action(String label, View.OnClickListener l) {
        Button b = new Button(this);
        b.setText(label);
        b.setAllCaps(false);
        b.setTextSize(12);
        b.setTextColor(ACCENT);
        b.setBackground(rounded(0xFF10222B, dp(10), 0xFF1B3A46));
        b.setStateListAnimator(null);
        b.setOnClickListener(l);
        return b;
    }

    private LinearLayout.LayoutParams weight(boolean leftGap) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        if (leftGap) lp.leftMargin = dp(8);
        return lp;
    }

    // -------------------------------------------------------------- action

    private Integer probe(String host, int port) {
        long t0 = System.currentTimeMillis();
        java.net.Socket s = new java.net.Socket();
        try {
            s.connect(new java.net.InetSocketAddress(host, port), 4000);
            return (int) (System.currentTimeMillis() - t0);
        } catch (Exception e) {
            return null;
        } finally {
            try {
                s.close();
            } catch (Exception ignored) {
            }
        }
    }

    private static final String[] CLIENTS = {
            "com.v2ray.ang",           // v2rayNG
            "com.hiddify.hiddify",     // Hiddify
            "io.nekohasekai.sagernet", // SagerNet
            "moe.nb4a",                // NekoBox
    };

    private void openInClient(String link) {
        copy(link);
        for (String pkg : CLIENTS) {
            Intent i = getPackageManager().getLaunchIntentForPackage(pkg);
            if (i != null) {
                toast("链接已复制，去客户端里导入");
                startActivity(i);
                return;
            }
        }
        toast("未找到已安装的客户端，链接已复制");
    }

    private void copy(String text) {
        ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm != null) cm.setPrimaryClip(ClipData.newPlainText("veil", text));
    }

    private void share(String text) {
        Intent i = new Intent(Intent.ACTION_SEND);
        i.setType("text/plain");
        i.putExtra(Intent.EXTRA_TEXT, text);
        startActivity(Intent.createChooser(i, "分享节点"));
    }

    // -------------------------------------------------------------- helpers

    private GradientDrawable rounded(int fill, int radius, int stroke) {
        GradientDrawable d = new GradientDrawable();
        d.setColor(fill);
        d.setCornerRadius(radius);
        if (stroke != 0) d.setStroke(1, stroke);
        return d;
    }

    private int dp(int v) {
        return (int) TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v,
                getResources().getDisplayMetrics());
    }

    private void toast(String s) {
        Toast.makeText(this, s, Toast.LENGTH_SHORT).show();
    }

    private void hideKeyboard() {
        InputMethodManager im = (InputMethodManager) getSystemService(INPUT_METHOD_SERVICE);
        if (im != null && getCurrentFocus() != null) {
            im.hideSoftInputFromWindow(getCurrentFocus().getWindowToken(), 0);
        }
    }

    private int colorFor(String protocol) {
        if ("VLESS".equals(protocol)) return 0xFF2BB3FF;
        if ("Trojan".equals(protocol)) return 0xFFFFB020;
        if ("VMess".equals(protocol)) return 0xFFB388FF;
        if ("Hysteria2".equals(protocol)) return ACCENT;
        return DIM;
    }

    private int withAlpha(int color, int alpha) {
        return (alpha << 24) | (color & 0x00FFFFFF);
    }

    // -------------------------------------------------------------- adapter

    private class NodeAdapter extends BaseAdapter {

        @Override public int getCount() { return nodes.size(); }

        @Override public Object getItem(int i) { return nodes.get(i); }

        @Override public long getItemId(int i) { return i; }

        @Override
        public View getView(int position, View convert, ViewGroup parent) {
            Node n = nodes.get(position);
            LinearLayout row = new LinearLayout(MainActivity.this);
            row.setOrientation(LinearLayout.HORIZONTAL);
            row.setGravity(Gravity.CENTER_VERTICAL);
            row.setBackground(rounded(CARD, dp(16), BORDER));
            row.setPadding(dp(13), dp(12), dp(13), dp(12));

            int c = colorFor(n.protocol);
            TextView badge = new TextView(MainActivity.this);
            badge.setText(n.badge());
            badge.setTextColor(c);
            badge.setTextSize(11);
            badge.setTypeface(Typeface.DEFAULT_BOLD);
            badge.setGravity(Gravity.CENTER);
            badge.setBackground(rounded(withAlpha(c, 0x1F), dp(11), withAlpha(c, 0x59)));
            row.addView(badge, new LinearLayout.LayoutParams(dp(44), dp(38)));

            LinearLayout textBox = new LinearLayout(MainActivity.this);
            textBox.setOrientation(LinearLayout.VERTICAL);
            LinearLayout.LayoutParams textLp =
                    new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
            textLp.leftMargin = dp(12);
            row.addView(textBox, textLp);

            TextView name = new TextView(MainActivity.this);
            name.setText(n.name);
            name.setTextColor(TEXT);
            name.setTextSize(14);
            name.setSingleLine(true);
            name.setEllipsize(TextUtils.TruncateAt.END);
            textBox.addView(name);

            TextView meta = new TextView(MainActivity.this);
            meta.setText(n.transport.toUpperCase(Locale.ROOT) + " · " + n.endpoint());
            meta.setTextColor(DIM);
            meta.setTextSize(11);
            meta.setSingleLine(true);
            meta.setEllipsize(TextUtils.TruncateAt.END);
            textBox.addView(meta);

            TextView chip = new TextView(MainActivity.this);
            if (n.latency == null) {
                chip.setText("测");
                chip.setTextColor(0xFF4E5C69);
                chip.setBackground(rounded(0x1A4E5C69, dp(9), 0x474E5C69));
            } else {
                int lc = n.latency < 120 ? ACCENT : (n.latency < 300 ? 0xFFFFB020 : 0xFFFF6B6B);
                chip.setText(n.latency + " ms");
                chip.setTextColor(lc);
                chip.setBackground(rounded(withAlpha(lc, 0x1A), dp(9), withAlpha(lc, 0x47)));
            }
            chip.setTextSize(11);
            chip.setTypeface(Typeface.DEFAULT_BOLD);
            chip.setGravity(Gravity.CENTER);
            chip.setPadding(dp(8), dp(4), dp(8), dp(4));
            row.addView(chip);

            row.setOnLongClickListener(v -> {
                n.latency = probe(n.host, n.port);
                notifyDataSetChanged();
                return true;
            });
            return row;
        }
    }
}
