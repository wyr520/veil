import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../models.dart';
import '../state.dart';

class NodeDetailPage extends StatelessWidget {
  const NodeDetailPage({super.key, required this.node});

  final ProxyNode node;

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final latency = state.latencyOf(node);

    return Scaffold(
      appBar: AppBar(
        title: Text(node.name, overflow: TextOverflow.ellipsis),
        actions: [
          IconButton(
            tooltip: '删除',
            onPressed: () async {
              await state.removeNode(node);
              if (context.mounted) Navigator.of(context).pop();
            },
            icon: const Icon(Icons.delete_outline_rounded),
          ),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(20, 8, 20, 32),
        children: [
          Center(
            child: Container(
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                color: Colors.white,
                borderRadius: BorderRadius.circular(20),
              ),
              child: QrImageView(
                data: node.uri,
                version: QrVersions.auto,
                size: 220,
                backgroundColor: Colors.white,
                errorStateBuilder: (_, __) => const SizedBox(
                  width: 220,
                  height: 220,
                  child: Center(child: Text('链接过长，无法生成二维码')),
                ),
              ),
            ),
          ),
          const SizedBox(height: 10),
          Center(
            child: Text(
              '用另一台设备扫码导入',
              style: TextStyle(color: Colors.white.withOpacity(0.35), fontSize: 12),
            ),
          ),
          const SizedBox(height: 22),
          _Section(
            title: '连接信息',
            children: [
              _Row(label: '协议', value: node.protocol.label),
              _Row(label: '传输', value: node.transport.toUpperCase()),
              _Row(label: '地址', value: node.endpoint),
              if (node.sni != null) _Row(label: 'SNI', value: node.sni!),
              if (node.path != null) _Row(label: '路径', value: node.path!),
              if (node.uuid != null) _Row(label: 'UUID', value: node.uuid!, mono: true),
              if (node.password != null) _Row(label: '密码', value: node.password!, mono: true),
              _Row(label: 'TLS 校验', value: node.insecure ? '跳过（自签证书）' : '正常'),
              _Row(
                label: 'TCP 延迟',
                value: latency == null ? '未测' : '$latency ms',
                valueColor: latency == null
                    ? null
                    : latency < 120
                        ? const Color(0xFF3DDC97)
                        : latency < 300
                            ? const Color(0xFFFFB020)
                            : const Color(0xFFFF6B6B),
              ),
            ],
          ),
          const SizedBox(height: 18),
          Row(
            children: [
              Expanded(
                child: OutlinedButton.icon(
                  onPressed: () async {
                    await Clipboard.setData(ClipboardData(text: node.uri));
                    if (context.mounted) {
                      ScaffoldMessenger.of(context).showSnackBar(
                        const SnackBar(content: Text('分享链接已复制')),
                      );
                    }
                  },
                  icon: const Icon(Icons.copy_all_rounded, size: 18),
                  label: const Text('复制链接'),
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: FilledButton.icon(
                  onPressed: () => state.testNode(node),
                  icon: const Icon(Icons.speed_rounded, size: 18),
                  label: const Text('测延迟'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 22),
          const _HandoffHint(),
        ],
      ),
    );
  }
}

class _Section extends StatelessWidget {
  const _Section({required this.title, required this.children});

  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: const Color(0xFF141A22),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: const Color(0xFF1F2630)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 14, 16, 8),
            child: Text(
              title,
              style: const TextStyle(
                color: Color(0xFF6B7885),
                fontSize: 12,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.8,
              ),
            ),
          ),
          ...children,
          const SizedBox(height: 6),
        ],
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.label,
    required this.value,
    this.mono = false,
    this.valueColor,
  });

  final String label;
  final String value;
  final bool mono;
  final Color? valueColor;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 9),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 82,
            child: Text(
              label,
              style: const TextStyle(color: Color(0xFF6B7885), fontSize: 13),
            ),
          ),
          Expanded(
            child: Text(
              value,
              style: TextStyle(
                color: valueColor ?? const Color(0xFFD6E2EC),
                fontSize: 13,
                fontFamily: mono ? 'monospace' : null,
                height: 1.35,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _HandoffHint extends StatelessWidget {
  const _HandoffHint();

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: const Color(0xFF10222B),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: const Color(0xFF1B3A46)),
      ),
      child: const Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.info_outline_rounded, size: 18, color: Color(0xFF2BB3FF)),
          SizedBox(width: 10),
          Expanded(
            child: Text(
              'Veil 客户端以本地 SOCKS5/HTTP 代理方式工作。需要全局代理时，'
              '把上面这条链接导入 v2rayNG / Hiddify / sing-box 等已支持 TUN 的客户端即可。',
              style: TextStyle(color: Color(0xFF8FA6B4), fontSize: 12, height: 1.6),
            ),
          ),
        ],
      ),
    );
  }
}
