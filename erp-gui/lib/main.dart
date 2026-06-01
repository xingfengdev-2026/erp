import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import 'models.dart';
import 'native_bridge.dart';
import 'profile_store.dart';
import 'share_codec.dart';
import 'socks5_server.dart';

void main() {
  runApp(const ErpGuiApp());
}

class ErpGuiApp extends StatelessWidget {
  const ErpGuiApp({super.key});

  @override
  Widget build(BuildContext context) {
    const seed = Color(0xff1f6f5b);
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      title: 'erp',
      theme: ThemeData(
        useMaterial3: true,
        colorScheme: ColorScheme.fromSeed(
          seedColor: seed,
          brightness: Brightness.light,
        ),
        scaffoldBackgroundColor: const Color(0xfff4f7f2),
        cardTheme: CardThemeData(
          elevation: 0,
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        ),
      ),
      darkTheme: ThemeData(
        useMaterial3: true,
        colorScheme: ColorScheme.fromSeed(
          seedColor: seed,
          brightness: Brightness.dark,
        ),
        cardTheme: CardThemeData(
          elevation: 0,
          shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        ),
      ),
      home: const HomePage(),
    );
  }
}

class HomePage extends StatefulWidget {
  const HomePage({super.key});

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  final _store = ProfileStore();
  final _share = ShareCodec();
  final _socks = Socks5Server();
  final _bridge = NativeErpBridge();
  final _logs = <String>[];
  StreamSubscription<String>? _socksSub;
  StreamSubscription<String>? _bridgeSub;

  var _profiles = <ErpProfile>[];
  var _selectedIndex = 0;
  var _loading = true;
  var _erpRunning = false;

  ErpProfile get _profile => _profiles[_selectedIndex];

  @override
  void initState() {
    super.initState();
    _load();
    _socksSub = _socks.events.listen(_log);
    _bridgeSub = _bridge.events.listen((message) {
      _log(message);
      if (message.startsWith('erp client exited') && mounted) {
        setState(() => _erpRunning = false);
      }
    });
  }

  @override
  void dispose() {
    _socksSub?.cancel();
    _bridgeSub?.cancel();
    unawaited(_socks.stop());
    unawaited(_bridge.dispose());
    super.dispose();
  }

  Future<void> _load() async {
    final profiles = await _store.load();
    setState(() {
      _profiles = profiles;
      _loading = false;
    });
  }

  Future<void> _persist() => _store.save(_profiles);

  void _replaceSelected(ErpProfile profile) {
    setState(() {
      _profiles[_selectedIndex] = profile;
    });
    unawaited(_persist());
  }

  void _log(String message) {
    if (!mounted) return;
    setState(() {
      final time = TimeOfDay.now().format(context);
      _logs.insert(0, '$time  $message');
      if (_logs.length > 80) _logs.removeLast();
    });
  }

  Future<void> _toggleSocks() async {
    if (_socks.running) {
      await _socks.stop();
    } else {
      await _socks.start(port: _profile.socks5Port);
    }
    if (mounted) setState(() {});
  }

  Future<void> _toggleErp() async {
    try {
      if (_erpRunning) {
        await _bridge.stopClient();
        _log('erp client stopped');
      } else {
        await _bridge.startClient(_profile);
      }
      if (mounted) {
        setState(() => _erpRunning = _bridge.state == ErpRuntimeState.running);
      }
    } catch (error) {
      _log('erp client failed: $error');
    }
  }

  Future<void> _copyToml() async {
    await Clipboard.setData(ClipboardData(text: _bridge.renderToml(_profile)));
    _log('client TOML copied');
  }

  Future<void> _addProfile() async {
    final profile = ErpProfile.starter().copyWith(
      id: DateTime.now().microsecondsSinceEpoch.toString(),
      name: 'Android SOCKS5 ${_profiles.length + 1}',
    );
    setState(() {
      _profiles.add(profile);
      _selectedIndex = _profiles.length - 1;
    });
    await _persist();
  }

  Future<void> _deleteProfile() async {
    if (_profiles.length == 1) return;
    setState(() {
      _profiles.removeAt(_selectedIndex);
      _selectedIndex = 0;
    });
    await _persist();
  }

  @override
  Widget build(BuildContext context) {
    if (_loading) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }
    return Scaffold(
      body: SafeArea(
        child: CustomScrollView(
          slivers: [
            SliverToBoxAdapter(child: _HeroHeader(profile: _profile)),
            SliverPadding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 24),
              sliver: SliverList.list(
                children: [
                  _profileSelector(),
                  const SizedBox(height: 12),
                  _statusPanel(),
                  const SizedBox(height: 12),
                  _mappingPanel(),
                  const SizedBox(height: 12),
                  _sharePanel(),
                  const SizedBox(height: 12),
                  _logPanel(),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _profileSelector() {
    return Row(
      children: [
        Expanded(
          child: DropdownButtonFormField<int>(
            initialValue: _selectedIndex,
            decoration: const InputDecoration(
              labelText: 'Profile',
              border: OutlineInputBorder(),
            ),
            items: [
              for (var i = 0; i < _profiles.length; i++)
                DropdownMenuItem(value: i, child: Text(_profiles[i].name)),
            ],
            onChanged: (value) => setState(() => _selectedIndex = value ?? 0),
          ),
        ),
        const SizedBox(width: 8),
        IconButton.filledTonal(
          tooltip: 'Add profile',
          onPressed: _addProfile,
          icon: const Icon(Icons.add),
        ),
        const SizedBox(width: 8),
        IconButton.filledTonal(
          tooltip: 'Edit profile',
          onPressed: _editProfile,
          icon: const Icon(Icons.tune),
        ),
        if (_profiles.length > 1) ...[
          const SizedBox(width: 8),
          IconButton.filledTonal(
            tooltip: 'Delete profile',
            onPressed: _deleteProfile,
            icon: const Icon(Icons.delete_outline),
          ),
        ],
      ],
    );
  }

  Widget _statusPanel() {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                _StatusPill(
                  label: _socks.running
                      ? 'SOCKS5 ${_socks.tcpPort}'
                      : 'SOCKS5 off',
                  active: _socks.running,
                ),
                const SizedBox(width: 8),
                _StatusPill(
                  label: _erpRunning ? 'erp running' : 'erp stopped',
                  active: _erpRunning,
                ),
              ],
            ),
            const SizedBox(height: 14),
            Text(
              _profile.serverAddr,
              style: Theme.of(
                context,
              ).textTheme.titleLarge?.copyWith(fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 4),
            Text(
              'client_id: ${_profile.clientId}  •  ${_profile.transport.wire}',
            ),
            const SizedBox(height: 16),
            Row(
              children: [
                Expanded(
                  child: FilledButton.icon(
                    onPressed: _toggleSocks,
                    icon: Icon(_socks.running ? Icons.stop : Icons.lan),
                    label: Text(
                      _socks.running ? 'Stop SOCKS5' : 'Start SOCKS5',
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: FilledButton.tonalIcon(
                    onPressed: _toggleErp,
                    icon: Icon(_erpRunning ? Icons.pause : Icons.play_arrow),
                    label: Text(_erpRunning ? 'Stop erp' : 'Start erp'),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            SizedBox(
              width: double.infinity,
              child: OutlinedButton.icon(
                onPressed: _copyToml,
                icon: const Icon(Icons.description_outlined),
                label: const Text('Copy client TOML'),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _mappingPanel() {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    'Mappings',
                    style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                TextButton.icon(
                  onPressed: _addMapping,
                  icon: const Icon(Icons.add),
                  label: const Text('Add'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            for (final mapping in _profile.mappings)
              _MappingTile(
                mapping: mapping,
                onEdit: () => _editMapping(mapping),
                onDelete: () {
                  final mappings = _profile.mappings
                      .where((m) => m != mapping)
                      .toList();
                  _replaceSelected(_profile.copyWith(mappings: mappings));
                },
              ),
          ],
        ),
      ),
    );
  }

  Widget _sharePanel() {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Share',
              style: Theme.of(
                context,
              ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 10),
            Row(
              children: [
                Expanded(
                  child: OutlinedButton.icon(
                    onPressed: _showExport,
                    icon: const Icon(Icons.qr_code_2),
                    label: const Text('Export QR/link'),
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: OutlinedButton.icon(
                    onPressed: _showImport,
                    icon: const Icon(Icons.input),
                    label: const Text('Import erp://'),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _logPanel() {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Activity',
              style: Theme.of(
                context,
              ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 8),
            if (_logs.isEmpty) const Text('No activity yet.'),
            for (final log in _logs.take(8))
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 3),
                child: Text(log, style: Theme.of(context).textTheme.bodySmall),
              ),
          ],
        ),
      ),
    );
  }

  Future<void> _editProfile() async {
    final edited = await showDialog<ErpProfile>(
      context: context,
      builder: (_) => ProfileDialog(profile: _profile),
    );
    if (edited != null) _replaceSelected(edited);
  }

  Future<void> _addMapping() async {
    final mapping = await showDialog<ErpMapping>(
      context: context,
      builder: (_) => MappingDialog(
        mapping: ErpMapping(
          name: 'socks5',
          protocol: ErpProtocol.tcp,
          localAddr: '127.0.0.1:${_profile.socks5Port}',
          remotePort: 18080,
        ),
      ),
    );
    if (mapping != null) {
      _replaceSelected(
        _profile.copyWith(mappings: [..._profile.mappings, mapping]),
      );
    }
  }

  Future<void> _editMapping(ErpMapping mapping) async {
    final edited = await showDialog<ErpMapping>(
      context: context,
      builder: (_) => MappingDialog(mapping: mapping),
    );
    if (edited != null) {
      final mappings = _profile.mappings
          .map((m) => m == mapping ? edited : m)
          .toList();
      _replaceSelected(_profile.copyWith(mappings: mappings));
    }
  }

  Future<void> _showExport() async {
    final passphrase = await _askPassphrase('Encrypt profile');
    if (passphrase == null || passphrase.isEmpty) return;
    final link = await _share.encode(_profile, passphrase);
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Export profile'),
        content: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              QrImageView(data: link, size: 220, backgroundColor: Colors.white),
              const SizedBox(height: 12),
              SelectableText(link),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () {
              Clipboard.setData(ClipboardData(text: link));
              Navigator.pop(context);
              _log('erp:// link copied');
            },
            child: const Text('Copy link'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Close'),
          ),
        ],
      ),
    );
  }

  Future<void> _showImport() async {
    final linkController = TextEditingController();
    final passController = TextEditingController();
    final profile = await showDialog<ErpProfile>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Import profile'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: linkController,
              minLines: 3,
              maxLines: 5,
              decoration: const InputDecoration(labelText: 'erp:// link'),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: passController,
              obscureText: true,
              decoration: const InputDecoration(labelText: 'Passphrase'),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () async {
              final navigator = Navigator.of(dialogContext);
              final messenger = ScaffoldMessenger.of(dialogContext);
              try {
                final imported = await _share.decode(
                  linkController.text,
                  passController.text,
                );
                navigator.pop(imported);
              } catch (error) {
                messenger.showSnackBar(
                  SnackBar(content: Text('Import failed: $error')),
                );
              }
            },
            child: const Text('Import'),
          ),
        ],
      ),
    );
    if (profile != null) {
      setState(() {
        _profiles.add(
          profile.copyWith(
            id: DateTime.now().microsecondsSinceEpoch.toString(),
          ),
        );
        _selectedIndex = _profiles.length - 1;
      });
      await _persist();
      _log('Imported ${profile.name}');
    }
  }

  Future<String?> _askPassphrase(String title) {
    final controller = TextEditingController();
    return showDialog<String>(
      context: context,
      builder: (_) => AlertDialog(
        title: Text(title),
        content: TextField(
          controller: controller,
          obscureText: true,
          decoration: const InputDecoration(labelText: 'Passphrase'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, controller.text),
            child: const Text('Continue'),
          ),
        ],
      ),
    );
  }
}

class _HeroHeader extends StatelessWidget {
  const _HeroHeader({required this.profile});

  final ErpProfile profile;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 24, 20, 20),
      decoration: BoxDecoration(
        gradient: LinearGradient(
          colors: [scheme.primary, scheme.tertiaryContainer],
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.hub, color: Colors.white, size: 30),
              const SizedBox(width: 10),
              Text(
                'erp',
                style: Theme.of(context).textTheme.headlineMedium?.copyWith(
                  color: Colors.white,
                  fontWeight: FontWeight.w800,
                ),
              ),
            ],
          ),
          const SizedBox(height: 16),
          Text(
            profile.name,
            style: Theme.of(context).textTheme.titleLarge?.copyWith(
              color: Colors.white,
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(height: 4),
          Text(
            'Expose Android SOCKS5 through a portable erp client profile.',
            style: Theme.of(context).textTheme.bodyMedium?.copyWith(
              color: Colors.white.withValues(alpha: 0.86),
            ),
          ),
        ],
      ),
    );
  }
}

class _StatusPill extends StatelessWidget {
  const _StatusPill({required this.label, required this.active});

  final String label;
  final bool active;

  @override
  Widget build(BuildContext context) {
    final color = active ? Colors.green : Theme.of(context).colorScheme.outline;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(99),
        border: Border.all(color: color),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.circle, size: 9, color: color),
          const SizedBox(width: 6),
          Text(label),
        ],
      ),
    );
  }
}

class _MappingTile extends StatelessWidget {
  const _MappingTile({
    required this.mapping,
    required this.onEdit,
    required this.onDelete,
  });

  final ErpMapping mapping;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final udp = mapping.udpMode == null ? '' : ' • ${mapping.udpMode!.wire}';
    return ListTile(
      contentPadding: EdgeInsets.zero,
      leading: Icon(
        mapping.protocol == ErpProtocol.tcp
            ? Icons.settings_ethernet
            : Icons.radar,
      ),
      title: Text(mapping.name),
      subtitle: Text(
        '${mapping.localAddr}  →  :${mapping.remotePort}  •  ${mapping.protocol.wire}$udp',
      ),
      trailing: Wrap(
        spacing: 4,
        children: [
          IconButton(
            tooltip: 'Edit',
            onPressed: onEdit,
            icon: const Icon(Icons.edit),
          ),
          IconButton(
            tooltip: 'Delete',
            onPressed: onDelete,
            icon: const Icon(Icons.delete_outline),
          ),
        ],
      ),
    );
  }
}

class ProfileDialog extends StatefulWidget {
  const ProfileDialog({super.key, required this.profile});

  final ErpProfile profile;

  @override
  State<ProfileDialog> createState() => _ProfileDialogState();
}

class _ProfileDialogState extends State<ProfileDialog> {
  late final TextEditingController name;
  late final TextEditingController serverAddr;
  late final TextEditingController clientId;
  late final TextEditingController token;
  late final TextEditingController socksPort;
  late ErpTransport transport;

  @override
  void initState() {
    super.initState();
    name = TextEditingController(text: widget.profile.name);
    serverAddr = TextEditingController(text: widget.profile.serverAddr);
    clientId = TextEditingController(text: widget.profile.clientId);
    token = TextEditingController(text: widget.profile.token);
    socksPort = TextEditingController(
      text: widget.profile.socks5Port.toString(),
    );
    transport = widget.profile.transport;
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Profile settings'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: name,
              decoration: const InputDecoration(labelText: 'Name'),
            ),
            TextField(
              controller: serverAddr,
              decoration: const InputDecoration(labelText: 'Server address'),
            ),
            TextField(
              controller: clientId,
              decoration: const InputDecoration(labelText: 'Client id'),
            ),
            TextField(
              controller: token,
              obscureText: true,
              decoration: const InputDecoration(labelText: 'Token'),
            ),
            TextField(
              controller: socksPort,
              keyboardType: TextInputType.number,
              decoration: const InputDecoration(labelText: 'Local SOCKS5 port'),
            ),
            const SizedBox(height: 12),
            DropdownButtonFormField<ErpTransport>(
              initialValue: transport,
              decoration: const InputDecoration(labelText: 'Transport'),
              items: [
                for (final t in ErpTransport.values)
                  DropdownMenuItem(value: t, child: Text(t.wire)),
              ],
              onChanged: (value) =>
                  setState(() => transport = value ?? ErpTransport.raw),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        TextButton(
          onPressed: () => Navigator.pop(context, null),
          child: const Text('Delete'),
        ),
        FilledButton(
          onPressed: () {
            Navigator.pop(
              context,
              widget.profile.copyWith(
                name: name.text,
                serverAddr: serverAddr.text,
                clientId: clientId.text,
                token: token.text,
                transport: transport,
                socks5Port: int.tryParse(socksPort.text) ?? 1080,
              ),
            );
          },
          child: const Text('Save'),
        ),
      ],
    );
  }
}

class MappingDialog extends StatefulWidget {
  const MappingDialog({super.key, required this.mapping});

  final ErpMapping mapping;

  @override
  State<MappingDialog> createState() => _MappingDialogState();
}

class _MappingDialogState extends State<MappingDialog> {
  late final TextEditingController name;
  late final TextEditingController localAddr;
  late final TextEditingController remotePort;
  late ErpProtocol protocol;
  late ErpUdpMode udpMode;

  @override
  void initState() {
    super.initState();
    name = TextEditingController(text: widget.mapping.name);
    localAddr = TextEditingController(text: widget.mapping.localAddr);
    remotePort = TextEditingController(
      text: widget.mapping.remotePort.toString(),
    );
    protocol = widget.mapping.protocol;
    udpMode = widget.mapping.udpMode ?? ErpUdpMode.overTcp;
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Mapping'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: name,
              decoration: const InputDecoration(labelText: 'Name'),
            ),
            DropdownButtonFormField<ErpProtocol>(
              initialValue: protocol,
              decoration: const InputDecoration(labelText: 'Protocol'),
              items: [
                for (final p in ErpProtocol.values)
                  DropdownMenuItem(value: p, child: Text(p.wire)),
              ],
              onChanged: (value) =>
                  setState(() => protocol = value ?? ErpProtocol.tcp),
            ),
            if (protocol == ErpProtocol.udp)
              DropdownButtonFormField<ErpUdpMode>(
                initialValue: udpMode,
                decoration: const InputDecoration(labelText: 'UDP mode'),
                items: [
                  for (final m in ErpUdpMode.values)
                    DropdownMenuItem(value: m, child: Text(m.wire)),
                ],
                onChanged: (value) =>
                    setState(() => udpMode = value ?? ErpUdpMode.overTcp),
              ),
            TextField(
              controller: localAddr,
              decoration: const InputDecoration(labelText: 'Local address'),
            ),
            TextField(
              controller: remotePort,
              keyboardType: TextInputType.number,
              decoration: const InputDecoration(labelText: 'Remote port'),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () {
            Navigator.pop(
              context,
              ErpMapping(
                name: name.text,
                protocol: protocol,
                localAddr: localAddr.text,
                remotePort: int.tryParse(remotePort.text) ?? 18080,
                udpMode: protocol == ErpProtocol.udp ? udpMode : null,
              ),
            );
          },
          child: const Text('Save'),
        ),
      ],
    );
  }
}
