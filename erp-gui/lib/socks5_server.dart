import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

class Socks5Server {
  ServerSocket? _tcpServer;
  RawDatagramSocket? _udpRelay;
  final _events = StreamController<String>.broadcast();

  Stream<String> get events => _events.stream;
  bool get running => _tcpServer != null;
  int? get tcpPort => _tcpServer?.port;
  int? get udpPort => _udpRelay?.port;

  Future<void> start({required int port}) async {
    if (running) return;
    _tcpServer = await ServerSocket.bind(InternetAddress.loopbackIPv4, port);
    _events.add('SOCKS5 listening on 127.0.0.1:${_tcpServer!.port}');
    _tcpServer!.listen(
      _handleClient,
      onError: (Object error) {
        _events.add('SOCKS5 TCP error: $error');
      },
    );
  }

  Future<void> stop() async {
    await _tcpServer?.close();
    _tcpServer = null;
    _udpRelay?.close();
    _udpRelay = null;
    _events.add('SOCKS5 stopped');
  }

  Future<void> _handleClient(Socket client) async {
    final reader = _SocketReader(client);
    try {
      final methods = await reader.readExact(2);
      if (methods[0] != 0x05) {
        throw const FormatException('invalid socks version');
      }
      final nmethods = methods[1];
      await reader.readExact(nmethods);
      client.add([0x05, 0x00]);
      await client.flush();

      final head = await reader.readExact(4);
      if (head[0] != 0x05) {
        throw const FormatException('invalid request version');
      }
      final command = head[1];
      final atyp = head[3];
      final targetHost = await _readAddress(reader, atyp);
      final portBytes = await reader.readExact(2);
      final targetPort = (portBytes[0] << 8) | portBytes[1];

      if (command == 0x01) {
        await _connect(client, reader, targetHost, targetPort);
      } else if (command == 0x03) {
        await _udpAssociate(client);
      } else {
        _reply(client, 0x07);
        await client.close();
      }
    } catch (error) {
      _events.add('SOCKS5 client error: $error');
      await client.close();
    } finally {
      await reader.cancel();
    }
  }

  Future<void> _connect(
    Socket client,
    _SocketReader reader,
    String host,
    int port,
  ) async {
    try {
      final remote = await Socket.connect(
        host,
        port,
        timeout: const Duration(seconds: 10),
      );
      _reply(client, 0x00);
      await client.flush();
      _events.add('CONNECT $host:$port');
      reader.forwardTo(remote);
      _pipeSocket(remote, client);
    } catch (_) {
      _reply(client, 0x05);
      await client.close();
    }
  }

  Future<void> _udpAssociate(Socket client) async {
    _udpRelay ??= await RawDatagramSocket.bind(InternetAddress.loopbackIPv4, 0);
    _udpRelay!.listen((event) {
      if (event != RawSocketEvent.read) return;
      final datagram = _udpRelay!.receive();
      if (datagram == null) return;
      _handleUdpDatagram(datagram);
    });
    _reply(client, 0x00, port: _udpRelay!.port);
    _events.add('UDP ASSOCIATE on 127.0.0.1:${_udpRelay!.port}');
  }

  Future<void> _handleUdpDatagram(Datagram datagram) async {
    final data = datagram.data;
    if (data.length < 10 || data[2] != 0) return;
    var offset = 3;
    final atyp = data[offset++];
    final parsed = _parseUdpAddress(data, offset, atyp);
    if (parsed == null) return;
    offset = parsed.nextOffset;
    if (data.length < offset + 2) return;
    final port = (data[offset] << 8) | data[offset + 1];
    offset += 2;
    try {
      final remote = await RawDatagramSocket.bind(InternetAddress.anyIPv4, 0);
      remote.send(data.sublist(offset), InternetAddress(parsed.host), port);
      late StreamSubscription sub;
      sub = remote.listen((event) {
        if (event != RawSocketEvent.read) return;
        final response = remote.receive();
        if (response == null) return;
        final packet = <int>[
          0,
          0,
          0,
          1,
          ...response.address.rawAddress,
          (response.port >> 8) & 0xff,
          response.port & 0xff,
          ...response.data,
        ];
        _udpRelay?.send(packet, datagram.address, datagram.port);
        sub.cancel();
        remote.close();
      });
      Timer(const Duration(seconds: 5), () {
        sub.cancel();
        remote.close();
      });
    } catch (error) {
      _events.add('UDP relay error: $error');
    }
  }

  Future<String> _readAddress(_SocketReader reader, int atyp) async {
    if (atyp == 0x01) {
      final raw = await reader.readExact(4);
      return InternetAddress.fromRawAddress(Uint8List.fromList(raw)).address;
    }
    if (atyp == 0x03) {
      final len = (await reader.readExact(1))[0];
      return String.fromCharCodes(await reader.readExact(len));
    }
    if (atyp == 0x04) {
      final raw = await reader.readExact(16);
      return InternetAddress.fromRawAddress(Uint8List.fromList(raw)).address;
    }
    throw FormatException('unsupported address type $atyp');
  }

  _UdpAddress? _parseUdpAddress(Uint8List data, int offset, int atyp) {
    if (atyp == 0x01 && data.length >= offset + 4) {
      final host = InternetAddress.fromRawAddress(
        data.sublist(offset, offset + 4),
      ).address;
      return _UdpAddress(host, offset + 4);
    }
    if (atyp == 0x03 && data.length > offset) {
      final len = data[offset++];
      if (data.length < offset + len) return null;
      return _UdpAddress(
        String.fromCharCodes(data.sublist(offset, offset + len)),
        offset + len,
      );
    }
    return null;
  }

  void _reply(Socket socket, int code, {int port = 0}) {
    socket.add([
      0x05,
      code,
      0x00,
      0x01,
      127,
      0,
      0,
      1,
      (port >> 8) & 0xff,
      port & 0xff,
    ]);
  }

  void _pipeSocket(Socket source, Socket sink) {
    late StreamSubscription<List<int>> sub;
    sub = source.listen(
      sink.add,
      onError: (_) async {
        await sub.cancel();
        await sink.close();
      },
      onDone: () async {
        await sub.cancel();
        await sink.close();
      },
      cancelOnError: true,
    );
  }
}

class _UdpAddress {
  const _UdpAddress(this.host, this.nextOffset);
  final String host;
  final int nextOffset;
}

class _SocketReader {
  _SocketReader(Socket socket) {
    _sub = socket.listen(
      (chunk) {
        _buffer.addAll(chunk);
        _flush();
      },
      onError: (Object error, StackTrace stackTrace) {
        _error = error;
        _flush();
      },
      onDone: () {
        _closed = true;
        _flush();
      },
      cancelOnError: true,
    );
  }

  final _buffer = <int>[];
  final _waiters = <_ReadWaiter>[];
  late final StreamSubscription<List<int>> _sub;
  Object? _error;
  bool _closed = false;
  bool _released = false;

  Future<List<int>> readExact(int length) {
    if (_buffer.length >= length) {
      final out = _buffer.sublist(0, length);
      _buffer.removeRange(0, length);
      return Future.value(out);
    }
    if (_error != null) return Future.error(_error!);
    if (_closed) return Future.error(const SocketException('socket closed'));
    final waiter = _ReadWaiter(length);
    _waiters.add(waiter);
    return waiter.completer.future;
  }

  Future<void> cancel() => _released ? Future.value() : _sub.cancel();

  void forwardTo(Socket sink) {
    if (_buffer.isNotEmpty) {
      sink.add(_buffer);
      _buffer.clear();
    }
    _released = true;
    _sub
      ..onData(sink.add)
      ..onError((_) {
        sink.close();
      })
      ..onDone(() {
        sink.close();
      });
  }

  void _flush() {
    while (_waiters.isNotEmpty && _buffer.length >= _waiters.first.length) {
      final waiter = _waiters.removeAt(0);
      final out = _buffer.sublist(0, waiter.length);
      _buffer.removeRange(0, waiter.length);
      waiter.completer.complete(out);
    }
    if (_error != null || _closed) {
      final error = _error ?? const SocketException('socket closed');
      for (final waiter in _waiters) {
        waiter.completer.completeError(error);
      }
      _waiters.clear();
    }
  }
}

class _ReadWaiter {
  _ReadWaiter(this.length);
  final int length;
  final completer = Completer<List<int>>();
}
