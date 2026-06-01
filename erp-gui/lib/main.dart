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
    const seed = Color(0xff3f6f5f);
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      title: 'erp',
      theme: ThemeData(
        useMaterial3: true,
        colorScheme: ColorScheme.fromSeed(seedColor: seed),
        scaffoldBackgroundColor: const Color(0xfff7f8f5),
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
  var _busy = false;
  String? _runningProfileId;

  ErpProfile get _profile => _profiles[_selectedIndex];
  bool get _running => _runningProfileId != null;

  @override
  void initState() {
    super.initState();
    _load();
    _socksSub = _socks.events.listen(_log);
    _bridgeSub = _bridge.events.listen((message) {
      _log(message);
      if (message.startsWith('erp client exited') && mounted) {
        unawaited(_socks.stop());
        setState(() => _runningProfileId = null);
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

  void _log(String message) {
    if (!mounted) return;
    setState(() {
      final time = TimeOfDay.now().format(context);
      _logs.insert(0, '$time  $message');
      if (_logs.length > 60) _logs.removeLast();
    });
  }

  Future<void> _toggleConnection() async {
    if (_busy) return;
    setState(() => _busy = true);
    try {
      if (_running) {
        await _stopRuntime();
      } else {
        await _startRuntime(_profile);
      }
    } catch (error) {
      _log('connect failed: $error');
      await _socks.stop();
      await _bridge.stopClient();
      if (mounted) setState(() => _runningProfileId = null);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _startRuntime(ErpProfile profile) async {
    if (profile.exposesSocks5) {
      await _socks.stop();
      await _socks.start(port: profile.localPort);
    }
    await _bridge.startClient(profile);
    setState(() => _runningProfileId = profile.id);
    _log('connected ${profile.name}');
  }

  Future<void> _stopRuntime() async {
    await _bridge.stopClient();
    await _socks.stop();
    setState(() => _runningProfileId = null);
    _log('disconnected');
  }

  Future<void> _addProfile() async {
    final edited = await showDialog<ErpProfile>(
      context: context,
      builder: (_) => ConfigEditorDialog(
        profile: ErpProfile.starter().copyWith(
          id: DateTime.now().microsecondsSinceEpoch.toString(),
          name: 'Config ${_profiles.length + 1}',
        ),
        title: 'Add config',
      ),
    );
    if (edited == null) return;
    setState(() {
      _profiles.add(edited);
      _selectedIndex = _profiles.length - 1;
    });
    await _persist();
  }

  Future<void> _editProfile(int index) async {
    final edited = await showDialog<ErpProfile>(
      context: context,
      builder: (_) =>
          ConfigEditorDialog(profile: _profiles[index], title: 'Edit config'),
    );
    if (edited == null) return;
    setState(() => _profiles[index] = edited);
    await _persist();
  }

  Future<void> _deleteProfile(int index) async {
    final deletingRunning = _profiles[index].id == _runningProfileId;
    if (deletingRunning) await _stopRuntime();
    setState(() {
      _profiles.removeAt(index);
      if (_profiles.isEmpty) {
        _profiles.add(ErpProfile.starter());
      }
      _selectedIndex = _selectedIndex.clamp(0, _profiles.length - 1);
    });
    await _persist();
  }

  Future<void> _showShare(ErpProfile profile) async {
    var encrypted = false;
    var link = _share.encodePlain(profile);
    final passController = TextEditingController();

    await showDialog<void>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) {
          final messenger = ScaffoldMessenger.of(dialogContext);
          Future<void> rebuildLink() async {
            try {
              final next = encrypted
                  ? await _share.encodeEncrypted(profile, passController.text)
                  : _share.encodePlain(profile);
              setDialogState(() => link = next);
            } catch (error) {
              messenger.showSnackBar(
                SnackBar(content: Text('Share failed: $error')),
              );
            }
          }

          return AlertDialog(
            title: Text('Share ${profile.name}'),
            content: SingleChildScrollView(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  SegmentedButton<bool>(
                    segments: const [
                      ButtonSegment(value: false, label: Text('Plain')),
                      ButtonSegment(value: true, label: Text('Encrypted')),
                    ],
                    selected: {encrypted},
                    onSelectionChanged: (values) async {
                      encrypted = values.first;
                      await rebuildLink();
                    },
                  ),
                  if (encrypted) ...[
                    const SizedBox(height: 12),
                    TextField(
                      controller: passController,
                      obscureText: true,
                      decoration: const InputDecoration(
                        labelText: 'Passphrase',
                        border: OutlineInputBorder(),
                      ),
                      onChanged: (_) => unawaited(rebuildLink()),
                    ),
                  ],
                  const SizedBox(height: 16),
                  QrImageView(
                    data: link,
                    size: 220,
                    backgroundColor: Colors.white,
                  ),
                  const SizedBox(height: 12),
                  SelectableText(link),
                ],
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(dialogContext),
                child: const Text('Close'),
              ),
              FilledButton.icon(
                onPressed: () {
                  Clipboard.setData(ClipboardData(text: link));
                  Navigator.pop(dialogContext);
                  _log('share link copied');
                },
                icon: const Icon(Icons.copy),
                label: const Text('Copy'),
              ),
            ],
          );
        },
      ),
    );
  }

  Future<void> _showImport() async {
    final linkController = TextEditingController();
    final passController = TextEditingController();
    final profile = await showDialog<ErpProfile>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text('Import erp://'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: linkController,
              minLines: 3,
              maxLines: 5,
              decoration: const InputDecoration(
                labelText: 'erp:// link',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: passController,
              obscureText: true,
              decoration: const InputDecoration(
                labelText: 'Passphrase if encrypted',
                border: OutlineInputBorder(),
              ),
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
                navigator.pop(
                  imported.copyWith(
                    id: DateTime.now().microsecondsSinceEpoch.toString(),
                  ),
                );
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
    if (profile == null) return;
    setState(() {
      _profiles.add(profile);
      _selectedIndex = _profiles.length - 1;
    });
    await _persist();
  }

  @override
  Widget build(BuildContext context) {
    if (_loading) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }

    final runningName = _profiles
        .where((profile) => profile.id == _runningProfileId)
        .map((profile) => profile.name)
        .firstOrNull;

    return Scaffold(
      appBar: AppBar(
        title: const Text('erp'),
        actions: [
          IconButton(
            tooltip: 'Add config',
            onPressed: _addProfile,
            icon: const Icon(Icons.add),
          ),
          PopupMenuButton<String>(
            tooltip: 'More',
            onSelected: (value) {
              if (value == 'import') unawaited(_showImport());
            },
            itemBuilder: (context) => const [
              PopupMenuItem(
                value: 'import',
                child: ListTile(
                  leading: Icon(Icons.input),
                  title: Text('Import erp://'),
                ),
              ),
            ],
          ),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _busy ? null : _toggleConnection,
        icon: Icon(_running ? Icons.link_off : Icons.link),
        label: Text(_running ? 'Disconnect' : 'Connect'),
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.fromLTRB(12, 8, 12, 96),
          children: [
            if (_running)
              _StatusBanner(
                label: 'Connected: ${runningName ?? 'unknown'}',
                active: true,
              )
            else
              const _StatusBanner(label: 'Disconnected', active: false),
            const SizedBox(height: 8),
            for (var i = 0; i < _profiles.length; i++)
              _ConfigTile(
                profile: _profiles[i],
                selected: i == _selectedIndex,
                running: _profiles[i].id == _runningProfileId,
                onTap: () => setState(() => _selectedIndex = i),
                onShare: () => _showShare(_profiles[i]),
                onEdit: () => _editProfile(i),
                onDelete: () => _deleteProfile(i),
              ),
            if (_logs.isNotEmpty) ...[
              const SizedBox(height: 12),
              _ActivityPanel(logs: _logs),
            ],
          ],
        ),
      ),
    );
  }
}

class _StatusBanner extends StatelessWidget {
  const _StatusBanner({required this.label, required this.active});

  final String label;
  final bool active;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
      decoration: BoxDecoration(
        color: active
            ? scheme.primaryContainer
            : scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        children: [
          Icon(active ? Icons.check_circle : Icons.radio_button_unchecked),
          const SizedBox(width: 10),
          Expanded(child: Text(label)),
        ],
      ),
    );
  }
}

class _ConfigTile extends StatelessWidget {
  const _ConfigTile({
    required this.profile,
    required this.selected,
    required this.running,
    required this.onTap,
    required this.onShare,
    required this.onEdit,
    required this.onDelete,
  });

  final ErpProfile profile;
  final bool selected;
  final bool running;
  final VoidCallback onTap;
  final VoidCallback onShare;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final mapping = profile.primaryMapping;
    final udp = mapping.udpMode == null ? '' : ' / ${mapping.udpMode!.wire}';
    return Card(
      margin: const EdgeInsets.symmetric(vertical: 5),
      color: selected ? scheme.secondaryContainer : null,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(8),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 4, 10),
          child: Row(
            children: [
              Icon(
                running
                    ? Icons.cloud_done
                    : selected
                    ? Icons.radio_button_checked
                    : Icons.radio_button_unchecked,
                color: running || selected ? scheme.primary : scheme.outline,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            profile.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: Theme.of(context).textTheme.titleMedium
                                ?.copyWith(fontWeight: FontWeight.w700),
                          ),
                        ),
                        const SizedBox(width: 6),
                        _KindChip(label: profile.kind.label),
                      ],
                    ),
                    const SizedBox(height: 3),
                    Text(
                      profile.serverAddr,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '${mapping.localAddr} -> :${mapping.remotePort}  ${mapping.protocol.wire}$udp',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                    Text(
                      '${profile.clientId} / ${profile.transport.wire}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
              IconButton(
                tooltip: 'Share',
                onPressed: onShare,
                icon: const Icon(Icons.qr_code_2),
              ),
              IconButton(
                tooltip: 'Edit',
                onPressed: onEdit,
                icon: const Icon(Icons.edit_outlined),
              ),
              IconButton(
                tooltip: 'Delete',
                onPressed: onDelete,
                icon: const Icon(Icons.delete_outline),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _KindChip extends StatelessWidget {
  const _KindChip({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        border: Border.all(color: Theme.of(context).colorScheme.outlineVariant),
        borderRadius: BorderRadius.circular(99),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        child: Text(label, style: Theme.of(context).textTheme.labelSmall),
      ),
    );
  }
}

class _ActivityPanel extends StatelessWidget {
  const _ActivityPanel({required this.logs});

  final List<String> logs;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Activity',
            style: Theme.of(
              context,
            ).textTheme.titleSmall?.copyWith(fontWeight: FontWeight.w700),
          ),
          const SizedBox(height: 6),
          for (final log in logs.take(6))
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Text(log, style: Theme.of(context).textTheme.bodySmall),
            ),
        ],
      ),
    );
  }
}

class ConfigEditorDialog extends StatefulWidget {
  const ConfigEditorDialog({
    super.key,
    required this.profile,
    required this.title,
  });

  final ErpProfile profile;
  final String title;

  @override
  State<ConfigEditorDialog> createState() => _ConfigEditorDialogState();
}

class _ConfigEditorDialogState extends State<ConfigEditorDialog> {
  late final TextEditingController name;
  late final TextEditingController serverAddr;
  late final TextEditingController clientId;
  late final TextEditingController token;
  late final TextEditingController localAddr;
  late final TextEditingController remotePort;
  late ErpProfileKind kind;
  late ErpTransport transport;
  late ErpProtocol protocol;
  late ErpUdpMode udpMode;

  @override
  void initState() {
    super.initState();
    final mapping = widget.profile.primaryMapping;
    name = TextEditingController(text: widget.profile.name);
    serverAddr = TextEditingController(text: widget.profile.serverAddr);
    clientId = TextEditingController(text: widget.profile.clientId);
    token = TextEditingController(text: widget.profile.token);
    localAddr = TextEditingController(text: mapping.localAddr);
    remotePort = TextEditingController(text: mapping.remotePort.toString());
    kind = widget.profile.kind;
    transport = widget.profile.transport;
    protocol = mapping.protocol;
    udpMode = mapping.udpMode ?? ErpUdpMode.overTcp;
  }

  @override
  void dispose() {
    name.dispose();
    serverAddr.dispose();
    clientId.dispose();
    token.dispose();
    localAddr.dispose();
    remotePort.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.title),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            SegmentedButton<ErpProfileKind>(
              segments: [
                for (final value in ErpProfileKind.values)
                  ButtonSegment(value: value, label: Text(value.label)),
              ],
              selected: {kind},
              onSelectionChanged: (values) {
                setState(() {
                  kind = values.first;
                  if (kind == ErpProfileKind.socks5) {
                    protocol = ErpProtocol.tcp;
                  }
                });
              },
            ),
            const SizedBox(height: 12),
            _Field(controller: name, label: 'Name'),
            _Field(controller: serverAddr, label: 'Server ip:port'),
            _Field(controller: clientId, label: 'Client id'),
            _Field(controller: token, label: 'Token', obscure: true),
            DropdownButtonFormField<ErpTransport>(
              initialValue: transport,
              decoration: const InputDecoration(
                labelText: 'Transport',
                border: OutlineInputBorder(),
              ),
              items: [
                for (final value in ErpTransport.values)
                  DropdownMenuItem(value: value, child: Text(value.wire)),
              ],
              onChanged: (value) =>
                  setState(() => transport = value ?? ErpTransport.raw),
            ),
            const SizedBox(height: 10),
            if (kind == ErpProfileKind.forwarding) ...[
              DropdownButtonFormField<ErpProtocol>(
                initialValue: protocol,
                decoration: const InputDecoration(
                  labelText: 'Protocol',
                  border: OutlineInputBorder(),
                ),
                items: [
                  for (final value in ErpProtocol.values)
                    DropdownMenuItem(value: value, child: Text(value.wire)),
                ],
                onChanged: (value) =>
                    setState(() => protocol = value ?? ErpProtocol.tcp),
              ),
              const SizedBox(height: 10),
            ],
            if (protocol == ErpProtocol.udp) ...[
              DropdownButtonFormField<ErpUdpMode>(
                initialValue: udpMode,
                decoration: const InputDecoration(
                  labelText: 'UDP mode',
                  border: OutlineInputBorder(),
                ),
                items: [
                  for (final value in ErpUdpMode.values)
                    DropdownMenuItem(value: value, child: Text(value.wire)),
                ],
                onChanged: (value) =>
                    setState(() => udpMode = value ?? ErpUdpMode.overTcp),
              ),
              const SizedBox(height: 10),
            ],
            _Field(
              controller: localAddr,
              label: kind == ErpProfileKind.socks5
                  ? 'Local SOCKS5 ip:port'
                  : 'Local ip:port',
            ),
            _Field(
              controller: remotePort,
              label: 'Remote port',
              keyboardType: TextInputType.number,
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
          onPressed: () => Navigator.pop(context, _buildProfile()),
          child: const Text('Save'),
        ),
      ],
    );
  }

  ErpProfile _buildProfile() {
    final selectedProtocol = kind == ErpProfileKind.socks5
        ? ErpProtocol.tcp
        : protocol;
    final mapping = ErpMapping(
      name: kind == ErpProfileKind.socks5 ? 'socks5' : name.text.trim(),
      protocol: selectedProtocol,
      localAddr: localAddr.text.trim(),
      remotePort: int.tryParse(remotePort.text.trim()) ?? 18080,
      udpMode: selectedProtocol == ErpProtocol.udp ? udpMode : null,
    );
    return widget.profile.copyWith(
      kind: kind,
      name: name.text.trim().isEmpty ? widget.profile.name : name.text.trim(),
      serverAddr: serverAddr.text.trim(),
      clientId: clientId.text.trim(),
      token: token.text,
      transport: transport,
      socks5Port: ErpProfile.parsePort(mapping.localAddr) ?? 1080,
      mappings: [mapping],
    );
  }
}

class _Field extends StatelessWidget {
  const _Field({
    required this.controller,
    required this.label,
    this.obscure = false,
    this.keyboardType,
  });

  final TextEditingController controller;
  final String label;
  final bool obscure;
  final TextInputType? keyboardType;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: TextField(
        controller: controller,
        obscureText: obscure,
        keyboardType: keyboardType,
        decoration: InputDecoration(
          labelText: label,
          border: const OutlineInputBorder(),
        ),
      ),
    );
  }
}
