import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import 'sheet_test/pages.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  // Draw under the system bars; the sheet keeps its content clear of them.
  SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
  runApp(const MainApp());
}

class MainApp extends StatelessWidget {
  const MainApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp.router(
      debugShowCheckedModeBanner: false,
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: Colors.blueGrey)),
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: Colors.blueGrey, brightness: Brightness.dark),
      ),
      routerConfig: sheetTestRouter,
    );
  }
}
