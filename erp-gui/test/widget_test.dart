import 'package:erp_gui/main.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  testWidgets('renders erp Android shell', (tester) async {
    SharedPreferences.setMockInitialValues({});
    await tester.pumpWidget(const ErpGuiApp());
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('erp'), findsOneWidget);
    expect(find.text('Mappings'), findsOneWidget);

    await tester.drag(find.text('Mappings'), const Offset(0, -500));
    await tester.pump();

    expect(find.text('Share'), findsOneWidget);
  });
}
