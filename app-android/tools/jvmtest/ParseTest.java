import com.veil.app.Node;

/**
 * Runs the share-link parser on a plain JVM (android.jar only provides the
 * classes it links against). Catches the things that are easy to get wrong and
 * impossible to notice until the app is on a phone: port defaults, IPv6
 * literals, percent-encoded remarks, '+' inside passwords.
 */
public class ParseTest {

    static int failed = 0;

    static void check(String what, Object got, Object want) {
        boolean ok = got == null ? want == null : got.equals(want);
        if (!ok) {
            failed++;
            System.out.println("FAIL " + what + "\n  got  " + got + "\n  want " + want);
        } else {
            System.out.println("ok   " + what + " = " + got);
        }
    }

    public static void main(String[] a) {
        Node n;

        n = Node.parse("vless://11111111-2222-3333-4444-555555555555@node.example.com:443"
                + "?encryption=none&security=tls&type=tcp&sni=node.example.com&fp=chrome#veil-node-vless-tls");
        check("vless.host", n.host, "node.example.com");
        check("vless.port", n.port, 443);
        check("vless.uuid", n.uuid, "11111111-2222-3333-4444-555555555555");
        check("vless.transport", n.transport, "tcp");
        check("vless.sni", n.sni, "node.example.com");
        check("vless.name", n.name, "veil-node-vless-tls");

        n = Node.parse("vless://u@h:8443?type=ws&path=/veil&sni=h.example.com"
                + "&host=h.example.com#%E8%8A%82%E7%82%B9%20A");
        check("ws.path", n.path, "/veil");
        check("ws.transport", n.transport, "ws");
        check("ws.cn-name", n.name, "节点 A");

        n = Node.parse("trojan://p%2Ba%2Fs%3D@1.2.3.4:8444?security=tls&type=tls#tr");
        check("trojan.host", n.host, "1.2.3.4");
        check("trojan.port", n.port, 8444);
        check("trojan.password", n.password, "p+a/s=");

        n = Node.parse("hysteria2://hunter2@hy.example.com:8446?sni=hy.example.com&insecure=1#hy2");
        check("hy2.protocol", n.protocol, "Hysteria2");
        check("hy2.transport", n.transport, "quic");
        check("hy2.password", n.password, "hunter2");

        n = Node.parse("hy2://pw@[2001:db8::1]:443?sni=x#v6");
        check("v6.host", n.host, "2001:db8::1");
        check("v6.port", n.port, 443);

        n = Node.parse("vless://u@noport.example.com?type=tcp#x");
        check("default.port", n.port, 443);

        check("garbage", Node.parse("not a link"), null);
        check("empty", Node.parse(""), null);

        System.out.println(failed == 0 ? "\nALL PASS" : "\n" + failed + " FAILED");
        System.exit(failed == 0 ? 0 : 1);
    }
}
