import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'models.dart';

enum ErpRuntimeState { stopped, running }

class NativeErpBridge {
  final _events = StreamController<String>.broadcast();
  Process? _process;

  ErpRuntimeState state = ErpRuntimeState.stopped;
  Stream<String> get events => _events.stream;

  Future<void> startClient(ErpProfile profile) async {
    if (_process != null) return;
    if (Platform.isAndroid || Platform.isIOS) {
      throw UnsupportedError('Android native erp runtime is not bundled yet');
    }

    final configFile = await _writeTempConfig(profile);
    final executable = Platform.environment['ERP_BIN'] ?? _defaultExecutable();
    _process = await Process.start(executable, [
      'client',
      '--config',
      configFile.path,
    ]);
    state = ErpRuntimeState.running;
    _events.add('erp client process started with ${configFile.path}');
    _streamLines(_process!.stdout, 'erp');
    _streamLines(_process!.stderr, 'erp');
    unawaited(
      _process!.exitCode.then((code) {
        state = ErpRuntimeState.stopped;
        _process = null;
        _events.add('erp client exited with code $code');
      }),
    );
  }

  Future<void> stopClient() async {
    _process?.kill();
    _process = null;
    state = ErpRuntimeState.stopped;
  }

  Future<void> dispose() async {
    await stopClient();
    await _events.close();
  }

  String renderToml(ErpProfile profile) {
    final buffer = StringBuffer()
      ..writeln('role = "client"')
      ..writeln('token = "${_escape(profile.token)}"')
      ..writeln('transport = "${profile.transport.wire}"')
      ..writeln()
      ..writeln('[client]')
      ..writeln('server_addr = "${_escape(profile.serverAddr)}"')
      ..writeln('client_id = "${_escape(profile.clientId)}"')
      ..writeln();
    for (final mapping in profile.mappings) {
      buffer
        ..writeln('[[client.mappings]]')
        ..writeln('name = "${_escape(mapping.name)}"')
        ..writeln('protocol = "${mapping.protocol.wire}"')
        ..writeln('local_addr = "${_escape(mapping.localAddr)}"')
        ..writeln('remote_port = ${mapping.remotePort}');
      if (mapping.udpMode != null) {
        buffer.writeln('udp_mode = "${mapping.udpMode!.wire}"');
      }
      buffer.writeln();
    }
    return buffer.toString();
  }

  String _escape(String value) =>
      value.replaceAll(r'\', r'\\').replaceAll('"', r'\"');

  Future<File> _writeTempConfig(ErpProfile profile) async {
    final safeId = profile.id.replaceAll(RegExp(r'[^a-zA-Z0-9_.-]'), '_');
    final file = File(
      '${Directory.systemTemp.path}${Platform.pathSeparator}erp-gui-$safeId.toml',
    );
    return file.writeAsString(renderToml(profile));
  }

  String _defaultExecutable() => Platform.isWindows ? 'erp.exe' : 'erp';

  void _streamLines(Stream<List<int>> stream, String prefix) {
    stream.transform(utf8.decoder).transform(const LineSplitter()).listen((
      line,
    ) {
      if (line.trim().isNotEmpty) _events.add('$prefix: $line');
    });
  }
}
