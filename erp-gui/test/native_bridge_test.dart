import 'package:erp_gui/models.dart';
import 'package:erp_gui/native_bridge.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('SOCKS5 profile renders TCP and UDP runtime mappings', () {
    final toml = NativeErpBridge().renderToml(ErpProfile.starter());

    expect(toml, contains('name = "socks5-tcp"'));
    expect(toml, contains('protocol = "tcp"'));
    expect(toml, contains('name = "socks5-udp"'));
    expect(toml, contains('protocol = "udp"'));
    expect(toml, contains('udp_mode = "over_tcp"'));
  });
}
