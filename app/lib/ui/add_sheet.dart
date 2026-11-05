import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../state.dart';

Future<void> showAddSheet(BuildContext context) {
  return showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    backgroundColor: const Color(0xFF0F151C),
    shape: const RoundedRectangleBorder(
      borderRadius: BorderRadius.vertical(top: Radius.circular(22)),
    ),
    builder: (_) => const _AddSheet(),
  );
}

enum _Mode { subscription, links, server }

class _AddSheet extends StatefulWidget {
  const _AddSheet();

  @override
  State<_AddSheet> createState() => _AddSheetState();
}

class _AddSheetState extends State<_AddSheet> {
  _Mode _mode = _Mode.subscription;
  final _url = TextEditingController();
  final _links = TextEditingController();
  final _base = TextEditingController(text: 'https://');
  final _user = TextEditingController(text: 'default');
  bool _working = false;

  @override
  void dispose() {
    _url.dispose();
    _links.dispose();
    _base.dispose();
    _user.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    final state = context.read<AppState>();
    setState(() => _working = true);
    try {
      switch (_mode) {
        case _Mode.subscription:
          if (_url.text.trim().isEmpty) throw Exception('请填写订阅链接');
          await state.addSubscription(_url.text.trim());
          break;
        case _Mode.links:
          final n = state.addLinks(_links.text);
          if (n == 0) throw Exception('没有识别到有效的分享链接');
          break;
        case _Mode.server:
          if (_base.text.trim().isEmpty || _user.text.trim().isEmpty) {
            throw Exception('请填写服务器地址和用户名');
          }
          await state.addServer(_base.text.trim(), _user.text.trim());
          break;
      }
      if (mounted) Navigator.of(context).pop();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(e.toString().replaceFirst('Exception: ', ''))));
      }
    } finally {
      if (mounted) setState(() => _working = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final bottom = MediaQuery.of(context).viewInsets.bottom;
    return Padding(
      padding: EdgeInsets.fromLTRB(20, 18, 20, 20 + bottom),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Text(
                '添加节点',
                style: TextStyle(fontSize: 18, fontWeight: FontWeight.w700, color: Colors.white),
              ),
              const Spacer(),
              IconButton(
                onPressed: () => Navigator.of(context).pop(),
                icon: const Icon(Icons.close_rounded, color: Color(0xFF6B7885)),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            children: [
              _chip('订阅链接', _Mode.subscription),
              _chip('单条链接', _Mode.links),
              _chip('自建服务器', _Mode.server),
            ],
          ),
          const SizedBox(height: 18),
          if (_mode == _Mode.subscription) ...[
            TextField(
              controller: _url,
              keyboardType: TextInputType.url,
              decoration: const InputDecoration(
                labelText: '订阅地址',
                hintText: 'https://example.com/sub/xxxxx',
                prefixIcon: Icon(Icons.link_rounded, size: 20),
              ),
            ),
            const SizedBox(height: 10),
            const Text(
              '支持 base64 订阅、Clash/明文链接列表。',
              style: TextStyle(color: Color(0xFF6B7885), fontSize: 12),
            ),
          ] else if (_mode == _Mode.links) ...[
            TextField(
              controller: _links,
              maxLines: 6,
              minLines: 4,
              decoration: const InputDecoration(
                labelText: '分享链接',
                hintText: 'vless://...\ntrojan://...\nvmess://...\nhysteria2://...',
                alignLabelWithHint: true,
              ),
            ),
          ] else ...[
            TextField(
              controller: _base,
              keyboardType: TextInputType.url,
              decoration: const InputDecoration(
                labelText: '服务端地址',
                hintText: 'https://node.example.com',
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _user,
              decoration: const InputDecoration(labelText: '用户名'),
            ),
            const SizedBox(height: 10),
            const Text(
              '会自动请求 /api/nodes/<用户名> 拉取节点。',
              style: TextStyle(color: Color(0xFF6B7885), fontSize: 12),
            ),
          ],
          const SizedBox(height: 20),
          SizedBox(
            height: 50,
            child: FilledButton(
              onPressed: _working ? null : _submit,
              child: _working
                  ? const SizedBox(
                      width: 20,
                      height: 20,
                      child: CircularProgressIndicator(strokeWidth: 2.2),
                    )
                  : const Text('导入'),
            ),
          ),
          const SizedBox(height: 6),
        ],
      ),
    );
  }

  Widget _chip(String label, _Mode mode) {
    final selected = _mode == mode;
    return ChoiceChip(
      label: Text(label),
      selected: selected,
      onSelected: (_) => setState(() => _mode = mode),
      showCheckmark: false,
      selectedColor: const Color(0xFF1B3A2F),
      backgroundColor: const Color(0xFF141A22),
      side: BorderSide(
        color: selected ? const Color(0xFF3DDC97) : const Color(0xFF1F2630),
      ),
      labelStyle: TextStyle(
        color: selected ? const Color(0xFF3DDC97) : const Color(0xFF9AA8B5),
        fontSize: 13,
        fontWeight: FontWeight.w600,
      ),
    );
  }
}
