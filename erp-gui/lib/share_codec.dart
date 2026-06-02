import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';

import 'models.dart';

class ShareCodec {
  static final _cipher = AesGcm.with256bits();
  static final _hash = Sha256();
  static final _kdf = Pbkdf2(
    macAlgorithm: Hmac.sha256(),
    iterations: 120000,
    bits: 256,
  );

  Future<String> encodePlain(ErpProfile profile) async {
    final salt = _randomBytes(16);
    final nonce = _randomBytes(12);
    final issuedAt = DateTime.now().millisecondsSinceEpoch;
    final rounds = Random.secure().nextInt(255) + 1;
    final key = await _deriveTimedKey(issuedAt, salt, rounds);
    final box = await _cipher.encrypt(
      utf8.encode(jsonEncode(profile.toJson())),
      secretKey: key,
      nonce: nonce,
    );
    final payload = {
      'v': 3,
      'mode': 'timed',
      'kdf': 'sha256-time-rounds',
      'rounds': rounds,
      't': issuedAt,
      'alg': 'aes-256-gcm',
      'salt': _b64(salt),
      'nonce': _b64(nonce),
      'mac': _b64(box.mac.bytes),
      'data': _b64(box.cipherText),
    };
    return 'erp://import?payload=${Uri.encodeComponent(_b64(utf8.encode(jsonEncode(payload))))}';
  }

  Future<String> encodeEncrypted(ErpProfile profile, String passphrase) async {
    if (passphrase.isEmpty) {
      throw const FormatException('Passphrase is required');
    }
    final salt = _randomBytes(16);
    final nonce = _randomBytes(12);
    final key = await _kdf.deriveKey(
      secretKey: SecretKey(utf8.encode(passphrase)),
      nonce: salt,
    );
    final box = await _cipher.encrypt(
      utf8.encode(jsonEncode(profile.toJson())),
      secretKey: key,
      nonce: nonce,
    );
    final payload = {
      'v': 2,
      'mode': 'encrypted',
      'kdf': 'pbkdf2-sha256',
      'iter': 120000,
      'alg': 'aes-256-gcm',
      'salt': _b64(salt),
      'nonce': _b64(nonce),
      'mac': _b64(box.mac.bytes),
      'data': _b64(box.cipherText),
    };
    return 'erp://import?payload=${Uri.encodeComponent(_b64(utf8.encode(jsonEncode(payload))))}';
  }

  Future<String> encode(
    ErpProfile profile,
    String passphrase, {
    bool encrypted = true,
  }) {
    if (!encrypted) return encodePlain(profile);
    return encodeEncrypted(profile, passphrase);
  }

  Future<ErpProfile> decode(String link, [String passphrase = '']) async {
    final uri = Uri.parse(link.trim());
    if (uri.scheme != 'erp' || uri.host != 'import') {
      throw FormatException('Not an erp import link');
    }

    final plainParam = uri.queryParameters['plain'];
    if (plainParam != null) {
      return ErpProfile.fromJson(
        jsonDecode(utf8.decode(_unb64(plainParam))) as Map<String, dynamic>,
      );
    }

    final payloadParam = uri.queryParameters['payload'];
    if (payloadParam == null) {
      throw FormatException('Missing payload');
    }
    final payload =
        jsonDecode(utf8.decode(_unb64(payloadParam))) as Map<String, dynamic>;
    final mode = payload['mode'] as String? ?? 'encrypted';
    if (mode == 'timed') {
      final salt = _unb64(payload['salt'] as String);
      final nonce = _unb64(payload['nonce'] as String);
      final mac = Mac(_unb64(payload['mac'] as String));
      final data = _unb64(payload['data'] as String);
      final key = await _deriveTimedKey(
        _intValue(payload['t'], 0),
        salt,
        _intValue(payload['rounds'], 1).clamp(1, 255).toInt(),
      );
      final clear = await _cipher.decrypt(
        SecretBox(data, nonce: nonce, mac: mac),
        secretKey: key,
      );
      return ErpProfile.fromJson(
        jsonDecode(utf8.decode(clear)) as Map<String, dynamic>,
      );
    }

    if (mode != 'encrypted') {
      throw FormatException('Unsupported payload mode');
    }
    if (passphrase.isEmpty) {
      throw const FormatException('Passphrase is required');
    }

    final salt = _unb64(payload['salt'] as String);
    final nonce = _unb64(payload['nonce'] as String);
    final mac = Mac(_unb64(payload['mac'] as String));
    final data = _unb64(payload['data'] as String);
    final key = await _kdf.deriveKey(
      secretKey: SecretKey(utf8.encode(passphrase)),
      nonce: salt,
    );
    final clear = await _cipher.decrypt(
      SecretBox(data, nonce: nonce, mac: mac),
      secretKey: key,
    );
    return ErpProfile.fromJson(
      jsonDecode(utf8.decode(clear)) as Map<String, dynamic>,
    );
  }

  Future<SecretKey> _deriveTimedKey(
    int issuedAt,
    List<int> salt,
    int rounds,
  ) async {
    List<int> material = utf8.encode('erp-share-v3|$issuedAt|${_b64(salt)}');
    for (var i = 0; i < rounds; i++) {
      material = (await _hash.hash(material)).bytes;
    }
    return SecretKey(material);
  }

  List<int> _randomBytes(int length) {
    final random = Random.secure();
    return List<int>.generate(length, (_) => random.nextInt(256));
  }

  String _b64(List<int> bytes) => base64UrlEncode(bytes).replaceAll('=', '');

  Uint8List _unb64(String value) {
    final normalized = value.padRight(
      value.length + ((4 - value.length % 4) % 4),
      '=',
    );
    return base64Url.decode(normalized);
  }
}

int _intValue(Object? value, int fallback) {
  if (value is int) return value;
  if (value is String) return int.tryParse(value) ?? fallback;
  return fallback;
}
