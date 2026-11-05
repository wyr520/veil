import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models.dart';
import '../state.dart';
import 'add_sheet.dart';
import 'node_detail_page.dart';

class HomePage extends StatelessWidget {
  const HomePage({super.key});

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    return Scaffold(
      appBar: AppBar(
        title: const Row(
          children: [
            _Logo(),
            SizedBox(width: 10),
            Text('Veil'),
          ],
        ),
        actions: [
          IconButton(
            tooltip: '测试全部延迟',
            onPressed: state.nodes.isEmpty ? null : () => state.testAll(),
            icon: const Icon(Icons.speed_rounded),
          ),
          const SizedBox(width: 4),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: () => showAddSheet(context),
        icon: const Icon(Icons.add_rounded),
        label: const Text('添加节点'),
      ),
      body: !state.loaded
          ? const Center(child: CircularProgressIndicator())
          : state.nodes.isEmpty
              ? const _EmptyState()
              : _NodeList(state: state),
    );
  }
}

class _Logo extends StatelessWidget {
  const _Logo();

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 30,
      height: 30,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(9),
        gradient: const LinearGradient(
          colors: [Color(0xFF3DDC97), Color(0xFF2BB3FF)],
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
        ),
      ),
      child: const Icon(Icons.shield_moon_outlined, size: 18, color: Color(0xFF06121A)),
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState();

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 32),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const Icon(Icons.cloud_off_rounded, size: 56, color: Color(0xFF3A4653)),
            const SizedBox(height: 20),
            Text(
              '还没有节点',
              style: Theme.of(context).textTheme.titleMedium?.copyWith(
                    color: Colors.white,
                    fontWeight: FontWeight.w600,
                  ),
            ),
            const SizedBox(height: 8),
            const Text(
              '粘贴订阅链接，或者从自建服务端拉取节点。\n支持 VLESS / Trojan / VMess / Hysteria2。',
              textAlign: TextAlign.center,
              style: TextStyle(color: Color(0xFF6B7885), height: 1.6),
            ),
            const SizedBox(height: 24),
            FilledButton.icon(
              onPressed: () => showAddSheet(context),
              icon: const Icon(Icons.add_link_rounded),
              label: const Text('导入订阅'),
            ),
          ],
        ),
      ),
    );
  }
}

class _NodeList extends StatelessWidget {
  const _NodeList({required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    final grouped = <String, List<ProxyNode>>{};
    for (final node in state.nodes) {
      grouped.putIfAbsent(node.group ?? '未分组', () => []).add(node);
    }
    final groupNames = grouped.keys.toList();

    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 96),
      children: [
        _Summary(state: state),
        const SizedBox(height: 18),
        for (final name in groupNames) ...[
          _GroupHeader(
            name: name,
            count: grouped[name]!.length,
            onDelete: () {
              final group = _groupNamed(state, name);
              if (group != null) _confirmDelete(context, state, group);
            },
            onRefresh: () {
              final group = _groupNamed(state, name);
              if (group != null && group.url.isNotEmpty) {
                state.refreshGroup(group).catchError((Object e) {
                  _toast(context, '$e');
                });
              }
            },
          ),
          const SizedBox(height: 8),
          for (final node in grouped[name]!)
            Padding(
              padding: const EdgeInsets.only(bottom: 10),
              child: NodeCard(node: node),
            ),
          const SizedBox(height: 8),
        ],
      ],
    );
  }

  void _confirmDelete(BuildContext context, AppState state, SubscriptionGroup group) {
    showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('删除订阅'),
        content: Text('删除「${group.name}」及其 ${state.nodes.where((n) => n.group == group.name).length} 个节点？'),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')),
          FilledButton(
            onPressed: () {
              Navigator.pop(ctx);
              state.removeGroup(group);
            },
            child: const Text('删除'),
          ),
        ],
      ),
    );
  }
}

SubscriptionGroup? _groupNamed(AppState state, String name) {
  for (final g in state.groups) {
    if (g.name == name) return g;
  }
  return null;
}

void _toast(BuildContext context, String message) {
  ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(message)));
}

class _Summary extends StatelessWidget {
  const _Summary({required this.state});

  final AppState state;

  @override
  Widget build(BuildContext context) {
    final reachable = state.nodes
        .where((n) => (state.latencyOf(n) ?? 99999) < 99999)
        .length;
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(18),
        gradient: const LinearGradient(
          colors: [Color(0xFF14261F), Color(0xFF121B24)],
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
        ),
        border: Border.all(color: const Color(0xFF1F3A32)),
      ),
      child: Row(
        children: [
          _Metric(value: '${state.nodes.length}', label: '节点'),
          _divider(),
          _Metric(value: '${state.groups.length}', label: '订阅'),
          _divider(),
          _Metric(value: '$reachable', label: '可达'),
        ],
      ),
    );
  }

  Widget _divider() => Container(
        width: 1,
        height: 32,
        color: const Color(0xFF23343A),
        margin: const EdgeInsets.symmetric(horizontal: 18),
      );
}

class _Metric extends StatelessWidget {
  const _Metric({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          value,
          style: const TextStyle(
            fontSize: 22,
            fontWeight: FontWeight.w700,
            color: Colors.white,
            height: 1.1,
          ),
        ),
        const SizedBox(height: 2),
        Text(label, style: const TextStyle(color: Color(0xFF6B7885), fontSize: 12)),
      ],
    );
  }
}

class _GroupHeader extends StatelessWidget {
  const _GroupHeader({
    required this.name,
    required this.count,
    required this.onDelete,
    required this.onRefresh,
  });

  final String name;
  final int count;
  final VoidCallback onDelete;
  final VoidCallback onRefresh;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Expanded(
          child: Text.rich(
            TextSpan(
              text: name,
              style: const TextStyle(
                color: Color(0xFFB9C6D2),
                fontSize: 13,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.6,
              ),
              children: [
                TextSpan(
                  text: '  $count',
                  style: const TextStyle(color: Color(0xFF4E5C69), fontSize: 12),
                ),
              ],
            ),
            overflow: TextOverflow.ellipsis,
          ),
        ),
        IconButton(
          onPressed: onRefresh,
          iconSize: 18,
          visualDensity: VisualDensity.compact,
          tooltip: '刷新',
          icon: const Icon(Icons.refresh_rounded, color: Color(0xFF6B7885)),
        ),
        IconButton(
          onPressed: onDelete,
          iconSize: 18,
          visualDensity: VisualDensity.compact,
          tooltip: '删除',
          icon: const Icon(Icons.delete_outline_rounded, color: Color(0xFF6B7885)),
        ),
      ],
    );
  }
}

class NodeCard extends StatelessWidget {
  const NodeCard({super.key, required this.node});

  final ProxyNode node;

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final latency = state.latencyOf(node);
    return Container(
      decoration: BoxDecoration(
        color: const Color(0xFF141A22),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: const Color(0xFF1F2630)),
      ),
      child: Material(
        color: Colors.transparent,
        borderRadius: BorderRadius.circular(16),
        child: InkWell(
          borderRadius: BorderRadius.circular(16),
          onTap: () => Navigator.of(context).push(
            MaterialPageRoute(builder: (_) => NodeDetailPage(node: node)),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
            child: Row(
              children: [
                _Badge(node.protocol),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        node.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: Colors.white,
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        '${node.transport.toUpperCase()} · ${node.endpoint}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(color: Color(0xFF6B7885), fontSize: 12),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 8),
                _LatencyChip(latency: latency, onTap: () => state.testNode(node)),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge(this.protocol);

  final Protocol protocol;

  @override
  Widget build(BuildContext context) {
    final color = switch (protocol) {
      Protocol.vless => const Color(0xFF2BB3FF),
      Protocol.trojan => const Color(0xFFFFB020),
      Protocol.vmess => const Color(0xFFB388FF),
      Protocol.hysteria2 => const Color(0xFF3DDC97),
      Protocol.unknown => const Color(0xFF6B7885),
    };
    return Container(
      width: 46,
      height: 40,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: color.withOpacity(0.12),
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: color.withOpacity(0.35)),
      ),
      child: Text(
        protocol.badge,
        style: TextStyle(
          color: color,
          fontSize: 11,
          fontWeight: FontWeight.w800,
          letterSpacing: 0.4,
        ),
      ),
    );
  }
}

class _LatencyChip extends StatelessWidget {
  const _LatencyChip({required this.latency, required this.onTap});

  final int? latency;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final (color, text) = switch (latency) {
      null => (const Color(0xFF4E5C69), '—'),
      final v when v < 120 => (const Color(0xFF3DDC97), '$v ms'),
      final v when v < 300 => (const Color(0xFFFFB020), '$v ms'),
      final v => (const Color(0xFFFF6B6B), '$v ms'),
    };
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(9),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 5),
        decoration: BoxDecoration(
          color: color.withOpacity(0.10),
          borderRadius: BorderRadius.circular(9),
          border: Border.all(color: color.withOpacity(0.28)),
        ),
        child: Text(
          text,
          style: TextStyle(color: color, fontSize: 11.5, fontWeight: FontWeight.w700),
        ),
      ),
    );
  }
}
