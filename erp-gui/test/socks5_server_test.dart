import 'dart:async';
import 'dart:io';

import 'package:erp_gui/socks5_server.dart';
import 'package:flutter_test/flutter_test.dart';

Future<int> _freeTcpPort() async {
  final socket = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
  final port = socket.port;
  await socket.close();
  return port;
}

void main() {
  test('SOCKS5 UDP associate relays datagrams', () async {
    final echo = await RawDatagramSocket.bind(InternetAddress.loopbackIPv4, 0);
    final echoSub = echo.listen((event) {
      if (event != RawSocketEvent.read) return;
      final datagram = echo.receive();
      if (datagram == null) return;
      echo.send(datagram.data, datagram.address, datagram.port);
    });

    final server = Socks5Server();
    final port = await _freeTcpPort();
    await server.start(port: port);

    final control = await Socket.connect(InternetAddress.loopbackIPv4, port);
    final reader = _StreamReader(control);
    control.add([0x05, 0x01, 0x00]);
    await control.flush();
    expect(await reader.readExact(2), [0x05, 0x00]);

    control.add([0x05, 0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
    await control.flush();
    final reply = await reader.readExact(10);
    expect(reply[0], 0x05);
    expect(reply[1], 0x00);
    final udpPort = (reply[8] << 8) | reply[9];
    expect(udpPort, port);

    final client = await RawDatagramSocket.bind(
      InternetAddress.loopbackIPv4,
      0,
    );
    final packet = <int>[
      0,
      0,
      0,
      1,
      127,
      0,
      0,
      1,
      (echo.port >> 8) & 0xff,
      echo.port & 0xff,
      ...'ping'.codeUnits,
    ];
    client.send(packet, InternetAddress.loopbackIPv4, udpPort);

    final completer = Completer<List<int>>();
    final clientSub = client.listen((event) {
      if (event != RawSocketEvent.read) return;
      final datagram = client.receive();
      if (datagram == null) return;
      completer.complete(datagram.data);
    });
    final response = await completer.future.timeout(const Duration(seconds: 3));
    expect(String.fromCharCodes(response.sublist(response.length - 4)), 'ping');

    await clientSub.cancel();
    client.close();
    await reader.cancel();
    await control.close();
    await server.stop();
    await echoSub.cancel();
    echo.close();
  });
}

class _StreamReader {
  _StreamReader(Stream<List<int>> stream) : _iterator = StreamIterator(stream);

  final StreamIterator<List<int>> _iterator;
  final _buffer = <int>[];

  Future<List<int>> readExact(int length) async {
    while (_buffer.length < length) {
      if (!await _iterator.moveNext()) {
        throw const SocketException('socket closed');
      }
      _buffer.addAll(_iterator.current);
    }
    final out = _buffer.sublist(0, length);
    _buffer.removeRange(0, length);
    return out;
  }

  Future<void> cancel() => _iterator.cancel();
}
