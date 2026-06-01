import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

import 'models.dart';

class ProfileStore {
  static const _key = 'erp_profiles_v1';

  Future<List<ErpProfile>> load() async {
    final prefs = await SharedPreferences.getInstance();
    final raw = prefs.getString(_key);
    if (raw == null || raw.isEmpty) {
      return [ErpProfile.starter()];
    }
    final decoded = jsonDecode(raw) as List<dynamic>;
    return decoded
        .map((e) => ErpProfile.fromJson(Map<String, dynamic>.from(e as Map)))
        .toList();
  }

  Future<void> save(List<ErpProfile> profiles) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(
      _key,
      jsonEncode(profiles.map((p) => p.toJson()).toList()),
    );
  }
}
