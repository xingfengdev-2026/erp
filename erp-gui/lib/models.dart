import 'dart:convert';

enum ErpRole { server, client }

enum ErpTransport { raw, aes256Gcm, aes128Gcm }

enum ErpProtocol { tcp, udp }

enum ErpUdpMode { overTcp, direct }

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
    remotePort: json['remote_port'] as int? ?? 18080,
    udpMode: json['udp_mode'] == null
        ? null
        : ErpUdpModeWire.parse(json['udp_mode'] as String),
  );
}

class ErpProfile {
  const ErpProfile({
    required this.id,
    required this.name,
    required this.serverAddr,
    required this.clientId,
    required this.token,
    required this.transport,
    required this.mappings,
    required this.socks5Port,
  });

  final String id;
  final String name;
  final String serverAddr;
  final String clientId;
  final String token;
  final ErpTransport transport;
  final List<ErpMapping> mappings;
  final int socks5Port;

  ErpProfile copyWith({
    String? id,
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
    'name': name,
    'server_addr': serverAddr,
    'client_id': clientId,
    'token': token,
    'transport': transport.wire,
    'socks5_port': socks5Port,
    'mappings': mappings.map((m) => m.toJson()).toList(),
  };

  String toPrettyJson() => const JsonEncoder.withIndent('  ').convert(toJson());

  static ErpProfile fromJson(Map<String, dynamic> json) => ErpProfile(
    id:
        json['id'] as String? ??
        DateTime.now().microsecondsSinceEpoch.toString(),
    name: json['name'] as String? ?? 'ERP Client',
    serverAddr: json['server_addr'] as String? ?? '127.0.0.1:7000',
    clientId: json['client_id'] as String? ?? 'android',
    token: json['token'] as String? ?? '',
    transport: ErpTransportWire.parse(json['transport'] as String? ?? 'raw'),
    socks5Port: json['socks5_port'] as int? ?? 1080,
    mappings: (json['mappings'] as List<dynamic>? ?? const [])
        .map((e) => ErpMapping.fromJson(Map<String, dynamic>.from(e as Map)))
        .toList(),
  );

  static ErpProfile starter() => ErpProfile(
    id: DateTime.now().microsecondsSinceEpoch.toString(),
    name: 'Android SOCKS5',
    serverAddr: '1.2.3.4:7000',
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
}
