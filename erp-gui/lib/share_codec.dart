import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'models.dart';

class ShareCodec {
  Future<String> encodePlain(ErpProfile profile) async {
    final payload = gzip.encode(utf8.encode(jsonEncode(_toCompact(profile))));
    return 'erp://i/${_b64(payload)}';
  }

  Future<String> encode(
    ErpProfile profile,
    String passphrase, {
    bool encrypted = true,
  }) => encodePlain(profile);

  Future<ErpProfile> decode(String link, [String passphrase = '']) async {
    final uri = Uri.parse(link.trim());
    if (uri.scheme != 'erp' ||
        uri.host != 'i' ||
        uri.pathSegments.length != 1) {
      throw const FormatException('Not an erp import link');
    }
    final clear = gzip.decode(_unb64(uri.pathSegments.single));
    return _fromCompact(jsonDecode(utf8.decode(clear)) as Map<String, dynamic>);
  }

  Map<String, dynamic> _toCompact(ErpProfile profile) => {
    'v': 1,
    'k': profile.kind == ErpProfileKind.forwarding ? 'f' : 's',
    'n': profile.name,
    's': profile.serverAddr,
    'c': profile.clientId,
    't': profile.token,
    'x': profile.transport.wire,
    'p': profile.socks5Port,
    'm': profile.mappings
        .map(
          (mapping) => [
            mapping.name,
            mapping.protocol.wire,
            mapping.localAddr,
            mapping.remotePort,
            if (mapping.udpMode != null) mapping.udpMode!.wire,
          ],
        )
        .toList(),
  };

  ErpProfile _fromCompact(Map<String, dynamic> json) {
    if (_intValue(json['v'], 0) != 1) {
      throw const FormatException('Unsupported erp link version');
    }
    final kind = _string(json['k']) == 'f'
        ? ErpProfileKind.forwarding
        : ErpProfileKind.socks5;
    final socks5Port = _intValue(json['p'], 1080);
    final mappings = (_list(
      json['m'],
    )).map((value) => _mappingFromCompact(_list(value))).toList();
    return ErpProfile(
      id: DateTime.now().microsecondsSinceEpoch.toString(),
      kind: kind,
      name: _string(json['n'], 'ERP Client'),
      serverAddr: _string(json['s'], '127.0.0.1:7000'),
      clientId: _string(json['c'], 'android'),
      token: _string(json['t']),
      transport: ErpTransportWire.parse(_string(json['x'], 'raw')),
      socks5Port: socks5Port,
      mappings: mappings.isEmpty
          ? [ErpProfile.defaultMapping(kind, socks5Port)]
          : mappings,
    );
  }

  ErpMapping _mappingFromCompact(List<dynamic> values) => ErpMapping(
    name: _string(values.elementAtOrNull(0), 'mapping'),
    protocol: ErpProtocolWire.parse(_string(values.elementAtOrNull(1), 'tcp')),
    localAddr: _string(values.elementAtOrNull(2), '127.0.0.1:8080'),
    remotePort: _intValue(values.elementAtOrNull(3), 18080),
    udpMode: values.length > 4
        ? ErpUdpModeWire.parse(_string(values.elementAtOrNull(4), 'over_tcp'))
        : null,
  );

  String _b64(List<int> bytes) => base64UrlEncode(bytes).replaceAll('=', '');

  Uint8List _unb64(String value) {
    final normalized = value.padRight(
      value.length + ((4 - value.length % 4) % 4),
      '=',
    );
    return base64Url.decode(normalized);
  }
}

String _string(Object? value, [String fallback = '']) =>
    value is String ? value : fallback;

List<dynamic> _list(Object? value) => value is List ? value : const [];

int _intValue(Object? value, int fallback) {
  if (value is int) return value;
  if (value is String) return int.tryParse(value) ?? fallback;
  return fallback;
}
