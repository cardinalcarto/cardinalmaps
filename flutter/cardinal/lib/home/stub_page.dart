import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';

/// A top-level page still to be built: its title and nothing else.
class StubPage extends StatelessWidget {
  const StubPage({super.key, required this.title});

  final String title;

  static const _header = #stubHeader;

  @override
  Widget build(BuildContext context) => SheetContent(
    showsBottomBar: true,
    anchors: const [SheetAnchor.section(_header)],
    slivers: [
      SliverToBoxAdapter(
        child: SheetSection(
          id: _header,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 16),
            child: Text(
              title,
              style: Theme.of(context).textTheme.headlineSmall,
            ),
          ),
        ),
      ),
    ],
  );
}
