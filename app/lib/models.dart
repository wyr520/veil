import 'dart:convert';

/// Protocols the app can display / hand off to a tunnel core.
enum Protocol { vless, trojan, vmess, hysteria2, unknown }

extension ProtocolLabel on Protocol {
  String get label => switch (this) {
        Protocol.vless => 'VLESS',
        Protocol.trojan => 'Trojan',
        Protocol.vmess => 'VMess',
        Protocol.hysteria2 => 'Hysteria2',
        Protocol.unknown => '未知',
      };

  String get badge => switch (this) {
        Protocol.vless => 'VL',
        Protocol.trojan => 'TR',
        Protocol.vmess => 'VM',
        Protocol.hysteria2 => 'HY2',
        Protocol.unknown => '?',
      };
}

/// One proxy endpoint parsed from a share URI.
class ProxyNode {
  final String uri;
  final Protocol protocol;
  final String name;
  final String host;
  final int port;
  final String? uuid;
  final String? password;
  final String transport; // tcp | ws | quic
  final String? sni;
  final String? path;
  final bool insecure;
  final String? group;

  const ProxyNode({
    required this.uri,
    required this.protocol,
    required this.name,
    required this.host,
    required this.port,
    this.uuid,
    this.password,
    this.transport = 'tcp',
    this.sni,
    this.path,
    this.insecure = false,
    this.group,
  });

  String get endpoint => '$host:$port';

  ProxyNode copyWith({String? group}) => ProxyNode(
        uri: uri,
        protocol: protocol,
        name: name,
        host: host,
        port: port,
        uuid: uuid,
        password: password,
        transport: transport,
        sni: sni,
        path: path,
        insecure: insecure,
        group: group ?? this.group,
      );

  Map<String, dynamic> toJson() => {
        'uri': uri,
        'name': name,
        'group': group,
      };

  static ProxyNode? fromJson(Map<String, dynamic> json) {
    final uri = json['uri'] as String?;
    if (uri == null) return null;
    final node = parseShareUri(uri);
    return node?.copyWith(group: json['group'] as String?);
  }
}

/// Parse `vless://`, `trojan://`, `vmess://` and `hysteria2://` links.
ProxyNode? parseShareUri(String raw) {
  final uri = raw.trim();
  if (uri.isEmpty) return null;

  if (uri.startsWith('vmess://')) return _parseVmess(uri);
  if (uri.startsWith('vless://')) return _parseUrlStyle(uri, Protocol.vless);
  if (uri.startsWith('trojan://')) return _parseUrlStyle(uri, Protocol.trojan);
  if (uri.startsWith('hysteria2://') || uri.startsWith('hy2://')) {
    return _parseUrlStyle(uri, Protocol.hysteria2);
  }
  return null;
}

ProxyNode? _parseUrlStyle(String uri, Protocol protocol) {
  try {
    final isHy = protocol == Protocol.hysteria2;
    final normalized = uri
        .replaceFirst('hysteria2://', 'https://')
        .replaceFirst('hy2://', 'https://')
        .replaceFirst('vless://', 'https://')
        .replaceFirst('trojan://', 'https://');
    final parsed = Uri.parse(normalized);
    final query = parsed.queryParameters;
    final transport = query['type'] ??
        (isHy ? 'quic' : (protocol == Protocol.trojan ? 'tcp' : 'tcp'));
    return ProxyNode(
      uri: uri,
      protocol: protocol,
      name: Uri.decodeComponent(parsed.fragment).isEmpty
          ? parsed.host
          : Uri.decodeComponent(parsed.fragment),
      host: parsed.host,
      port: parsed.hasPort ? parsed.port : (isHy ? 443 : 443),
      uuid: protocol == Protocol.vless || protocol == Protocol.vmess
          ? parsed.userInfo
          : null,
      password: protocol != Protocol.vless ? parsed.userInfo : null,
      transport: transport,
      sni: query['sni'] ?? query['peer'],
      path: query['path'],
      insecure: query['allowInsecure'] == '1' || query['insecure'] == '1',
    );
  } catch (_) {
    return null;
  }
}

ProxyNode? _parseVmess(String uri) {
  try {
    var payload = uri.substring('vmess://'.length);
    payload = payload.replaceAll(RegExp(r'\s'), '');
    // tolerate url-safe base64 and missing padding
    payload = payload.replaceAll('-', '+').replaceAll('_', '/');
    while (payload.length % 4 != 0) {
      payload += '=';
    }
    final json = jsonDecode(utf8.decode(base64Decode(payload))) as Map<String, dynamic>;
    return ProxyNode(
      uri: uri,
      protocol: Protocol.vmess,
      name: (json['ps'] ?? json['add'] ?? 'vmess').toString(),
      host: json['add'].toString(),
      port: int.tryParse(json['port'].toString()) ?? 443,
      uuid: json['id']?.toString(),
      transport: (json['net'] ?? 'tcp').toString(),
      sni: json['sni']?.toString(),
      path: json['path']?.toString(),
      insecure: json['allowInsecure'] == true || json['allowInsecure'] == '1',
    );
  } catch (_) {
    return null;
  }
}

/// Decode a subscription body (base64 of newline separated URIs).
List<ProxyNode> parseSubscription(String body) {
  var text = body.trim();
  if (!text.contains('://')) {
    final cleaned = text.replaceAll(RegExp(r'\s'), '');
    try {
      final padded = cleaned + '=' * ((4 - cleaned.length % 4) % 4);
      text = utf8.decode(base64Decode(padded.replaceAll('-', '+').replaceAll('_', '/')));
    } catch (_) {
      return const [];
    }
  }
  final nodes = <ProxyNode>[];
  for (final line in text.split(RegExp(r'[\r\n]+'))) {
    final node = parseShareUri(line);
    if (node != null) nodes.add(node);
  }
  return nodes;
}
