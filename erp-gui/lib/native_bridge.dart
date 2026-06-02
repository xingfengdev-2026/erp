import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';

import 'models.dart';

enum ErpRuntimeState { stopped, running }

class NativeErpBridge {
  static const _channel = MethodChannel('uk.nam2.erp/runtime');

  final _events = StreamController<String>.broadcast();
  Process? _process;

  ErpRuntimeState state = ErpRuntimeState.stopped;
  Stream<String> get events => _events.stream;

  Future<void> startClient(ErpProfile profile) async {
    if (_process != null) return;

    final configFile = await _writeTempConfig(profile);
    final executable = await _resolveExecutable();
    final ready = Completer<void>();
    _process = await Process.start(executable, [
      'client',
      '--config',
      configFile.path,
    ]);
    state = ErpRuntimeState.running;
    _events.add('erp client started: ${configFile.path}');
    _streamLines(
      _process!.stdout,
      'erp',
      onLine: (line) {
        if (line.trim() == 'client registered' && !ready.isCompleted) {
          ready.complete();
        }
      },
    );
    _streamLines(_process!.stderr, 'erp');
    final exitFuture = _process!.exitCode.then((code) {
      if (!ready.isCompleted) {
        ready.completeError(
          StateError('erp client exited before registration'),
        );
      }
      return code;
    });
    unawaited(
      exitFuture.then((code) {
        state = ErpRuntimeState.stopped;
        _process = null;
        _events.add('erp client exited with code $code');
      }),
    );
    try {
      await ready.future.timeout(
        const Duration(seconds: 12),
        onTimeout: () => throw TimeoutException('erp registration timed out'),
      );
    } catch (_) {
      await stopClient();
      rethrow;
    }
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

    for (final mapping in _runtimeMappings(profile)) {
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

  List<ErpMapping> _runtimeMappings(ErpProfile profile) {
    if (!profile.exposesSocks5) return profile.mappings;
    final mapping = profile.primaryMapping;
    return [
      ErpMapping(
        name: 'socks5-tcp',
        protocol: ErpProtocol.tcp,
        localAddr: mapping.localAddr,
        remotePort: mapping.remotePort,
      ),
      ErpMapping(
        name: 'socks5-udp',
        protocol: ErpProtocol.udp,
        localAddr: mapping.localAddr,
        remotePort: mapping.remotePort,
        udpMode: ErpUdpMode.overTcp,
      ),
    ];
  }

  Future<File> _writeTempConfig(ErpProfile profile) async {
    final safeId = profile.id.replaceAll(RegExp(r'[^a-zA-Z0-9_.-]'), '_');
    final file = File(
      '${Directory.systemTemp.path}${Platform.pathSeparator}erp-gui-$safeId.toml',
    );
    return file.writeAsString(renderToml(profile));
  }

  Future<String> _resolveExecutable() async {
    if (Platform.isAndroid) {
      final nativeDir = await _channel.invokeMethod<String>('nativeLibraryDir');
      if (nativeDir == null || nativeDir.isEmpty) {
        throw StateError('Android native library directory is unavailable');
      }
      final binary = File('$nativeDir${Platform.pathSeparator}liberp_exec.so');
      if (!await binary.exists()) {
        throw StateError('Bundled erp runtime was not found for this ABI');
      }
      return binary.path;
    }
    return Platform.environment['ERP_BIN'] ??
        (Platform.isWindows ? 'erp.exe' : 'erp');
  }

  String _escape(String value) =>
      value.replaceAll(r'\', r'\\').replaceAll('"', r'\"');

  void _streamLines(
    Stream<List<int>> stream,
    String prefix, {
    void Function(String line)? onLine,
  }) {
    stream.transform(utf8.decoder).transform(const LineSplitter()).listen((
      line,
    ) {
      if (line.trim().isNotEmpty) {
        _events.add('$prefix: $line');
        onLine?.call(line);
      }
    });
  }
}
