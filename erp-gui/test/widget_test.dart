import 'package:erp_gui/main.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  testWidgets('renders v2 style config shell', (tester) async {
    SharedPreferences.setMockInitialValues({});
    await tester.pumpWidget(const ErpGuiApp());
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('erp'), findsOneWidget);
    expect(find.byIcon(Icons.add), findsOneWidget);
    expect(find.text('Android SOCKS5'), findsOneWidget);
    expect(find.byIcon(Icons.qr_code_2), findsOneWidget);
    expect(find.text('Connect'), findsOneWidget);
  });
}
