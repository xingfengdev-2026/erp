import 'package:erp_gui/models.dart';
import 'package:erp_gui/share_codec.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('short erp link round trips profile', () async {
    final codec = ShareCodec();
    final profile = ErpProfile.starter();

    final link = await codec.encodePlain(profile);
    final decoded = await codec.decode(link);

    expect(link, startsWith('erp://i/'));
    expect(link.length, lessThan(360));
    expect(link, isNot(contains('Android')));
    expect(link, isNot(contains('android-phone')));
    expect(link, isNot(contains('127.0.0.1')));
    expect(link, isNot(contains('1080')));
    expect(decoded.name, profile.name);
    expect(
      decoded.primaryMapping.remotePort,
      profile.primaryMapping.remotePort,
    );
  });

  test('old erp import payload format is not accepted', () async {
    final codec = ShareCodec();

    await expectLater(
      codec.decode('erp://import?payload=abc'),
      throwsA(isA<FormatException>()),
    );
  });
}
