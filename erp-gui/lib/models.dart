import 'dart:convert';

enum ErpRole { server, client }

enum ErpProfileKind { socks5, forwarding }

enum ErpTransport { raw, aes256Gcm, aes128Gcm }

enum ErpProtocol { tcp, udp }

enum ErpUdpMode { overTcp, direct }

extension ErpProfileKindWire on ErpProfileKind {
  String get wire => switch (this) {
    ErpProfileKind.socks5 => 'socks5',
    ErpProfileKind.forwarding => 'forwarding',
  };

  String get label => switch (this) {
    ErpProfileKind.socks5 => 'SOCKS5 server',
    ErpProfileKind.forwarding => 'Forwarding node',
  };

  static ErpProfileKind parse(String? value) => switch (value) {
    'forwarding' => ErpProfileKind.forwarding,
    _ => ErpProfileKind.socks5,
  };
}

extension ErpTransportWire on ErpTransport {
  String get wire => switch (this) {
    ErpTransport.raw => 'raw',
    ErpTransport.aes256Gcm => 'aes-256-gcm',
    ErpTransport.aes128Gcm => 'aes-128-gcm',
  };

  static ErpTransport parse(String value) => switch (value) {
    'aes-256-gcm' => ErpTransport.aes256Gcm,
    'aes-128-gcm' => ErpTransport.aes128Gcm,
    _ => ErpTransport.raw,
  };
}

extension ErpProtocolWire on ErpProtocol {
  String get wire => switch (this) {
    ErpProtocol.tcp => 'tcp',
    ErpProtocol.udp => 'udp',
  };

  static ErpProtocol parse(String value) =>
      value == 'udp' ? ErpProtocol.udp : ErpProtocol.tcp;
}

extension ErpUdpModeWire on ErpUdpMode {
  String get wire => switch (this) {
    ErpUdpMode.overTcp => 'over_tcp',
    ErpUdpMode.direct => 'direct',
  };

  static ErpUdpMode parse(String value) =>
      value == 'direct' ? ErpUdpMode.direct : ErpUdpMode.overTcp;
}

class ErpMapping {
  const ErpMapping({
    required this.name,
    required this.protocol,
    required this.localAddr,
    required this.remotePort,
    this.udpMode,
  });

  final String name;
  final ErpProtocol protocol;
  final String localAddr;
  final int remotePort;
  final ErpUdpMode? udpMode;

  ErpMapping copyWith({
    String? name,
    ErpProtocol? protocol,
    String? localAddr,
    int? remotePort,
    ErpUdpMode? udpMode,
  }) {
    return ErpMapping(
      name: name ?? this.name,
      protocol: protocol ?? this.protocol,
      localAddr: localAddr ?? this.localAddr,
      remotePort: remotePort ?? this.remotePort,
      udpMode: udpMode ?? this.udpMode,
    );
  }

  Map<String, dynamic> toJson() => {
    'name': name,
    'protocol': protocol.wire,
    'local_addr': localAddr,
    'remote_port': remotePort,
    if (udpMode != null) 'udp_mode': udpMode!.wire,
  };

  static ErpMapping fromJson(Map<String, dynamic> json) => ErpMapping(
    name: json['name'] as String? ?? 'mapping',
    protocol: ErpProtocolWire.parse(json['protocol'] as String? ?? 'tcp'),
    localAddr: json['local_addr'] as String? ?? '127.0.0.1:8080',
    remotePort: _intValue(json['remote_port'], 18080),
    udpMode: json['udp_mode'] == null
        ? null
        : ErpUdpModeWire.parse(json['udp_mode'] as String),
  );
}

class ErpProfile {
  const ErpProfile({
    required this.id,
    required this.kind,
    required this.name,
    required this.serverAddr,
    required this.clientId,
    required this.token,
    required this.transport,
    required this.mappings,
    required this.socks5Port,
  });

  final String id;
  final ErpProfileKind kind;
  final String name;
  final String serverAddr;
  final String clientId;
  final String token;
  final ErpTransport transport;
  final List<ErpMapping> mappings;
  final int socks5Port;

  ErpMapping get primaryMapping =>
      mappings.isEmpty ? defaultMapping(kind, socks5Port) : mappings.first;

  int get localPort => parsePort(primaryMapping.localAddr) ?? socks5Port;

  bool get exposesSocks5 => kind == ErpProfileKind.socks5;

  ErpProfile copyWith({
    String? id,
    ErpProfileKind? kind,
    String? name,
    String? serverAddr,
    String? clientId,
    String? token,
    ErpTransport? transport,
    List<ErpMapping>? mappings,
    int? socks5Port,
  }) {
    return ErpProfile(
      id: id ?? this.id,
      kind: kind ?? this.kind,
      name: name ?? this.name,
      serverAddr: serverAddr ?? this.serverAddr,
      clientId: clientId ?? this.clientId,
      token: token ?? this.token,
      transport: transport ?? this.transport,
      mappings: mappings ?? this.mappings,
      socks5Port: socks5Port ?? this.socks5Port,
    );
  }

  Map<String, dynamic> toJson() => {
    'id': id,
    'kind': kind.wire,
    'name': name,
    'server_addr': serverAddr,
    'client_id': clientId,
    'token': token,
    'transport': transport.wire,
    'socks5_port': socks5Port,
    'mappings': mappings.map((m) => m.toJson()).toList(),
  };

  String toPrettyJson() => const JsonEncoder.withIndent('  ').convert(toJson());

  static ErpProfile fromJson(Map<String, dynamic> json) {
    final kind = ErpProfileKindWire.parse(json['kind'] as String?);
    final socks5Port = _intValue(json['socks5_port'], 1080);
    final mappings = (json['mappings'] as List<dynamic>? ?? const [])
        .map((e) => ErpMapping.fromJson(Map<String, dynamic>.from(e as Map)))
        .toList();

    return ErpProfile(
      id:
          json['id'] as String? ??
          DateTime.now().microsecondsSinceEpoch.toString(),
      kind: kind,
      name: json['name'] as String? ?? 'ERP Client',
      serverAddr: json['server_addr'] as String? ?? '127.0.0.1:7000',
      clientId: json['client_id'] as String? ?? 'android',
      token: json['token'] as String? ?? '',
      transport: ErpTransportWire.parse(json['transport'] as String? ?? 'raw'),
      socks5Port: socks5Port,
      mappings: mappings.isEmpty
          ? [defaultMapping(kind, socks5Port)]
          : mappings,
    );
  }

  static ErpProfile starter() => ErpProfile(
    id: DateTime.now().microsecondsSinceEpoch.toString(),
    kind: ErpProfileKind.socks5,
    name: 'Android SOCKS5',
    serverAddr: '127.0.0.1:7000',
    clientId: 'android-phone',
    token: 'change-me',
    transport: ErpTransport.raw,
    socks5Port: 1080,
    mappings: const [
      ErpMapping(
        name: 'socks5',
        protocol: ErpProtocol.tcp,
        localAddr: '127.0.0.1:1080',
        remotePort: 18080,
      ),
    ],
  );

  static ErpMapping defaultMapping(ErpProfileKind kind, int socks5Port) {
    if (kind == ErpProfileKind.socks5) {
      return ErpMapping(
        name: 'socks5',
        protocol: ErpProtocol.tcp,
        localAddr: '127.0.0.1:$socks5Port',
        remotePort: 18080,
      );
    }
    return const ErpMapping(
      name: 'forward',
      protocol: ErpProtocol.tcp,
      localAddr: '127.0.0.1:8080',
      remotePort: 18080,
    );
  }

  static int? parsePort(String address) {
    final index = address.lastIndexOf(':');
    if (index < 0 || index == address.length - 1) return null;
    return int.tryParse(address.substring(index + 1));
  }
}

int _intValue(Object? value, int fallback) {
  if (value is int) return value;
  if (value is String) return int.tryParse(value) ?? fallback;
  return fallback;
}
