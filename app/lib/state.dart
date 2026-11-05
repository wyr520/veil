import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'api.dart';
import 'models.dart';

class SubscriptionGroup {
  SubscriptionGroup({required this.name, required this.url, this.updatedAt});

  final String name;
  final String url;
  DateTime? updatedAt;

  Map<String, dynamic> toJson() => {
        'name': name,
        'url': url,
        'updatedAt': updatedAt?.toIso8601String(),
      };

  static SubscriptionGroup fromJson(Map<String, dynamic> json) => SubscriptionGroup(
        name: json['name'] as String? ?? '订阅',
        url: json['url'] as String? ?? '',
        updatedAt: json['updatedAt'] != null
            ? DateTime.tryParse(json['updatedAt'] as String)
            : null,
      );
}

/// App wide state: node list, subscriptions, latency cache.
class AppState extends ChangeNotifier {
  static const _kNodes = 'veil.nodes.v1';
  static const _kGroups = 'veil.groups.v1';

  final List<ProxyNode> _nodes = [];
  final List<SubscriptionGroup> _groups = [];
  final Map<String, int?> _latency = {};
  bool _busy = false;
  bool _loaded = false;

  List<ProxyNode> get nodes => List.unmodifiable(_nodes);
  List<SubscriptionGroup> get groups => List.unmodifiable(_groups);
  bool get busy => _busy;
  bool get loaded => _loaded;
  int? latencyOf(ProxyNode node) => _latency[node.uri];

  Future<void> load() async {
    final prefs = await SharedPreferences.getInstance();
    final rawNodes = prefs.getString(_kNodes);
    final rawGroups = prefs.getString(_kGroups);
    if (rawNodes != null) {
      try {
        final list = jsonDecode(rawNodes) as List<dynamic>;
        _nodes
          ..clear()
          ..addAll(list
              .map((e) => ProxyNode.fromJson(e as Map<String, dynamic>))
              .whereType<ProxyNode>());
      } catch (_) {/* corrupted cache, start clean */}
    }
    if (rawGroups != null) {
      try {
        final list = jsonDecode(rawGroups) as List<dynamic>;
        _groups
          ..clear()
          ..addAll(list.map((e) => SubscriptionGroup.fromJson(e as Map<String, dynamic>)));
      } catch (_) {}
    }
    _loaded = true;
    notifyListeners();
  }

  Future<void> _persist() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kNodes, jsonEncode(_nodes.map((n) => n.toJson()).toList()));
    await prefs.setString(_kGroups, jsonEncode(_groups.map((g) => g.toJson()).toList()));
  }

  void _replaceGroup(String group, List<ProxyNode> incoming) {
    _nodes.removeWhere((n) => n.group == group);
    for (final node in incoming) {
      if (!_nodes.any((n) => n.uri == node.uri)) {
        _nodes.add(node.copyWith(group: group));
      }
    }
  }

  Future<void> addSubscription(String url, {String? label}) async {
    _busy = true;
    notifyListeners();
    try {
      final nodes = await fetchSubscription(url);
      final name = label?.trim().isNotEmpty == true
          ? label!.trim()
          : (nodes.first.name.isNotEmpty ? nodes.first.name : '订阅');
      _replaceGroup(name, nodes);
      final existing = _groups.indexWhere((g) => g.url == url);
      if (existing >= 0) {
        _groups[existing].updatedAt = DateTime.now();
      } else {
        _groups.add(SubscriptionGroup(name: name, url: url, updatedAt: DateTime.now()));
      }
      await _persist();
    } finally {
      _busy = false;
      notifyListeners();
    }
  }

  Future<void> addServer(String baseUrl, String user) async {
    _busy = true;
    notifyListeners();
    try {
      final nodes = await fetchServerNodes(baseUrl, user);
      final name = 'veil:$user';
      _replaceGroup(name, nodes);
      if (!_groups.any((g) => g.name == name)) {
        _groups.add(SubscriptionGroup(
          name: name,
          url: '$baseUrl/api/nodes/$user',
          updatedAt: DateTime.now(),
        ));
      }
      await _persist();
    } finally {
      _busy = false;
      notifyListeners();
    }
  }

  /// Add pasted links (one per line).
  int addLinks(String text) {
    final added = <ProxyNode>[];
    for (final line in text.split(RegExp(r'[\r\n]+'))) {
      final node = parseShareUri(line);
      if (node != null && !_nodes.any((n) => n.uri == node.uri)) {
        added.add(node.copyWith(group: '手动'));
      }
    }
    if (added.isEmpty) return 0;
    _nodes.addAll(added);
    if (!_groups.any((g) => g.name == '手动')) {
      _groups.add(SubscriptionGroup(name: '手动', url: ''));
    }
    _persist();
    notifyListeners();
    return added.length;
  }

  Future<void> removeGroup(SubscriptionGroup group) async {
    _nodes.removeWhere((n) => n.group == group.name);
    _groups.remove(group);
    await _persist();
    notifyListeners();
  }

  Future<void> refreshGroup(SubscriptionGroup group) async {
    if (group.url.isEmpty) return;
    await addSubscription(group.url, label: group.name);
  }

  Future<void> removeNode(ProxyNode node) async {
    _nodes.removeWhere((n) => n.uri == node.uri);
    _latency.remove(node.uri);
    await _persist();
    notifyListeners();
  }

  Future<void> testAll() async {
    for (final node in _nodes) {
      _latency[node.uri] = await measureLatency(node.host, node.port);
      notifyListeners();
    }
  }

  Future<void> testNode(ProxyNode node) async {
    _latency[node.uri] = await measureLatency(node.host, node.port);
    notifyListeners();
  }
}
