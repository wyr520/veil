import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

import 'models.dart';

class ApiException implements Exception {
  final String message;
  ApiException(this.message);
  @override
  String toString() => message;
}

/// Downloads and decodes a subscription URL.
Future<List<ProxyNode>> fetchSubscription(String url) async {
  final uri = Uri.tryParse(url);
  if (uri == null || !uri.hasScheme) {
    throw ApiException('订阅链接格式不正确');
  }
  final client = http.Client();
  try {
    final request = http.Request('GET', uri)
      ..headers['User-Agent'] = 'veil-app/0.1 (v2rayN compatible)';
    final streamed = await client.send(request).timeout(const Duration(seconds: 20));
    final body = await streamed.stream.toBytes();
    if (streamed.statusCode != 200) {
      throw ApiException('服务器返回 ${streamed.statusCode}');
    }
    final nodes = parseSubscription(utf8.decode(body, allowMalformed: true));
    if (nodes.isEmpty) {
      throw ApiException('订阅内容里没有可识别的节点');
    }
    return nodes;
  } on TimeoutException {
    throw ApiException('请求超时');
  } on SocketException catch (e) {
    throw ApiException('网络错误: ${e.message}');
  } finally {
    client.close();
  }
}

/// Pull the node list straight from a veil server (`GET /api/nodes/<user>`).
Future<List<ProxyNode>> fetchServerNodes(String baseUrl, String user) async {
  final url = Uri.parse('${baseUrl.replaceAll(RegExp(r'/+$'), '')}/api/nodes/$user');
  final resp = await http
      .get(url)
      .timeout(const Duration(seconds: 15));
  if (resp.statusCode != 200) {
    throw ApiException('服务器返回 ${resp.statusCode}');
  }
  final list = jsonDecode(utf8.decode(resp.bodyBytes)) as List<dynamic>;
  final nodes = <ProxyNode>[];
  for (final item in list) {
    final uri = (item as Map<String, dynamic>)['uri'] as String?;
    if (uri == null) continue;
    final node = parseShareUri(uri);
    if (node != null) nodes.add(node);
  }
  if (nodes.isEmpty) {
    throw ApiException('服务器没有返回节点');
  }
  return nodes;
}

/// TCP round trip to the node. Cheap proxy for "can I reach it at all".
Future<int?> measureLatency(String host, int port) async {
  final watch = Stopwatch()..start();
  Socket? socket;
  try {
    socket = await Socket.connect(host, port, timeout: const Duration(seconds: 5));
    watch.stop();
    return watch.elapsedMilliseconds;
  } catch (_) {
    return null;
  } finally {
    socket?.destroy();
  }
}
