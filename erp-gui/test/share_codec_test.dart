import 'package:erp_gui/models.dart';
import 'package:erp_gui/share_codec.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('plain erp link round trips profile without passphrase', () async {
    final codec = ShareCodec();
    final profile = ErpProfile.starter();

    final link = codec.encodePlain(profile);
    final decoded = await codec.decode(link);

    expect(link, startsWith('erp://import?plain='));
    expect(decoded.name, profile.name);
    expect(
      decoded.primaryMapping.remotePort,
      profile.primaryMapping.remotePort,
    );
  });

  test('encrypted erp link requires matching passphrase', () async {
    final codec = ShareCodec();
    final profile = ErpProfile.starter();

    final link = await codec.encodeEncrypted(profile, 'secret-passphrase');
    final decoded = await codec.decode(link, 'secret-passphrase');

    expect(link, startsWith('erp://import?payload='));
    expect(decoded.serverAddr, profile.serverAddr);
    await expectLater(codec.decode(link, 'wrong'), throwsA(isA<Object>()));
  });
}
