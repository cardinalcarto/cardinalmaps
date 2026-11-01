// Temporary pages for exercising the sheet; to be removed as the app is built.

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';

/// The routes for the test pages: sheet pages in the shell, and one full screen page.
final sheetTestRouter = GoRouter(
  routes: [
    ShellRoute(
      builder: (context, state, child) => Scaffold(
        resizeToAvoidBottomInset: false,
        body: SheetScaffold(
          body: const _TestBody(),
          floating: const _FloatingProbe(),
          sheet: child,
        ),
      ),
      routes: [
        GoRoute(
          path: '/',
          pageBuilder: (context, state) =>
              SheetPage(key: state.pageKey, child: const SectionsTestPage()),
          routes: [
            GoRoute(
              path: 'short',
              pageBuilder: (context, state) =>
                  SheetPage(key: state.pageKey, child: const ShortTestPage()),
            ),
            GoRoute(
              path: 'long',
              pageBuilder: (context, state) =>
                  SheetPage(key: state.pageKey, child: const LongTestPage()),
            ),
            GoRoute(
              path: 'input',
              pageBuilder: (context, state) =>
                  SheetPage(key: state.pageKey, child: const InputTestPage()),
            ),
          ],
        ),
      ],
    ),
    GoRoute(path: '/full', builder: (context, state) => const FullScreenTestPage()),
  ],
);

/// Sections A and B each get an anchor; A's bar narrows as B comes into view.
class SectionsTestPage extends StatelessWidget {
  const SectionsTestPage({super.key});

  static const _a = #a;
  static const _b = #b;

  @override
  Widget build(BuildContext context) {
    return SheetContent(
      anchors: const [
        SheetAnchor.section(_a),
        SheetAnchor.section(_b),
        SheetAnchor.fraction(0.5),
        SheetAnchor.full(),
      ],
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _a,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
              child: SheetBuilder(
                child: const _Bar(label: 'Section A: narrows as B shows'),
                builder: (context, sheet, bar) {
                  final t = Curves.easeInOutCubic.transform(sheet.reveal(_b));
                  return Row(
                    children: [
                      Expanded(child: bar!),
                      ClipRect(
                        child: Align(
                          alignment: Alignment.centerRight,
                          widthFactor: t,
                          child: Padding(
                            padding: const EdgeInsets.only(left: 12),
                            child: Opacity(
                              opacity: t,
                              child: IconButton.filledTonal(
                                tooltip: 'Full screen page',
                                onPressed: () => context.push('/full'),
                                icon: const Icon(Icons.open_in_full),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ],
                  );
                },
              ),
            ),
          ),
        ),
        SliverToBoxAdapter(
          child: SheetSection(
            id: _b,
            child: Padding(
              padding: const EdgeInsets.only(bottom: 12),
              child: SizedBox(
                height: 40,
                child: ListView(
                  scrollDirection: Axis.horizontal,
                  padding: const EdgeInsets.symmetric(horizontal: 16),
                  children: [
                    for (final (label, path) in const [
                      ('Short page', '/short'),
                      ('Long page', '/long'),
                      ('Text input', '/input'),
                      ('Full screen page', '/full'),
                    ])
                      Padding(
                        padding: const EdgeInsets.only(right: 8),
                        child: ActionChip(label: Text(label), onPressed: () => context.push(path)),
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
        const SliverToBoxAdapter(child: _Readout(sections: [_a, _b])),
        const _NumberedTiles(count: 30),
      ],
    );
  }
}

/// Two anchors, both within its content: the sheet never goes above half or so, so the
/// content scrolls within a shorter viewport.
class ShortTestPage extends StatelessWidget {
  const ShortTestPage({super.key});

  static const _header = #header;
  static const _end = #end;

  @override
  Widget build(BuildContext context) {
    return SheetContent(
      anchors: const [SheetAnchor.section(_header), SheetAnchor.section(_end)],
      initialAnchor: 1,
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _header,
            child: const _Header(title: 'Short page', subtitle: 'Anchors at its header and end'),
          ),
        ),
        const SliverToBoxAdapter(child: _Readout(sections: [_header, _end])),
        SliverToBoxAdapter(
          child: SheetSection(
            id: _end,
            child: Column(children: [for (var i = 1; i <= 4; i++) ListTile(title: Text('Row $i'))]),
          ),
        ),
      ],
    );
  }
}

/// Fractional anchors with different gravities, a long list, and a title that changes
/// as the sheet turns into a full screen.
class LongTestPage extends StatelessWidget {
  const LongTestPage({super.key});

  static const _title = #title;

  @override
  Widget build(BuildContext context) {
    return SheetContent(
      anchors: const [
        SheetAnchor.section(_title, gravity: 0),
        SheetAnchor.fraction(0.4),
        SheetAnchor.fraction(0.7, gravity: 4000),
        SheetAnchor.full(),
      ],
      initialAnchor: 1,
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _title,
            child: SheetBuilder(
              builder: (context, sheet, _) {
                final e = sheet.expansion;
                return Row(
                  children: [
                    ClipRect(
                      child: Align(
                        alignment: Alignment.centerLeft,
                        widthFactor: e,
                        child: Opacity(
                          opacity: e,
                          child: const Padding(
                            padding: EdgeInsets.only(left: 4),
                            child: BackButton(),
                          ),
                        ),
                      ),
                    ),
                    const Expanded(
                      child: _Header(
                        title: 'Long page',
                        subtitle: 'Gravity: header 0, 40% default, 70% strong',
                      ),
                    ),
                    Opacity(
                      opacity: 1 - e,
                      child: const Padding(
                        padding: EdgeInsets.only(right: 8),
                        child: CloseButton(),
                      ),
                    ),
                  ],
                );
              },
            ),
          ),
        ),
        const SliverToBoxAdapter(child: _Readout(sections: [_title])),
        const _NumberedTiles(count: 100),
      ],
    );
  }
}

/// A text field, for the keyboard.
class InputTestPage extends StatelessWidget {
  const InputTestPage({super.key});

  @override
  Widget build(BuildContext context) {
    return SheetContent(
      anchors: const [SheetAnchor.fraction(0.5), SheetAnchor.full()],
      initialAnchor: 1,
      slivers: [
        const SliverToBoxAdapter(
          child: Padding(
            padding: EdgeInsets.fromLTRB(16, 4, 16, 12),
            child: TextField(
              autofocus: true,
              decoration: InputDecoration(
                filled: true,
                hintText: 'Type here',
                border: OutlineInputBorder(borderSide: BorderSide.none),
              ),
            ),
          ),
        ),
        const SliverToBoxAdapter(child: _Readout(sections: [])),
        const _NumberedTiles(count: 30),
      ],
    );
  }
}

/// A page outside the sheet, covering everything.
class FullScreenTestPage extends StatelessWidget {
  const FullScreenTestPage({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Full screen page')),
      body: const Center(child: Text('Not in the sheet')),
    );
  }
}

class _Bar extends StatelessWidget {
  const _Bar({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Material(
      color: theme.colorScheme.surfaceContainerHigh,
      shape: const StadiumBorder(),
      child: SizedBox(
        height: 56,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 20),
          child: Align(
            alignment: Alignment.centerLeft,
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.textTheme.bodyLarge,
            ),
          ),
        ),
      ),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.title, required this.subtitle});

  final String title;
  final String subtitle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: theme.textTheme.headlineSmall),
          Text(subtitle, style: theme.textTheme.bodyMedium),
        ],
      ),
    );
  }
}

/// Where the sheet is, live.
class _Readout extends StatelessWidget {
  const _Readout({required this.sections});

  final List<Symbol> sections;

  @override
  Widget build(BuildContext context) {
    final style = Theme.of(context).textTheme.bodySmall
        ?.copyWith(fontFeatures: const [FontFeature.tabularFigures()]);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
      child: SheetBuilder(
        builder: (context, sheet, _) {
          final controller = sheet.controller;
          final lines = [
            'extent ${sheet.extent.toStringAsFixed(1)}'
                '  expansion ${sheet.expansion.toStringAsFixed(2)}'
                '  ${sheet.geometry.placement.name}',
            'anchors ${controller.anchors.map((a) => a.extent.toStringAsFixed(0)).join(', ')}'
                '  resting ${controller.restingAt ?? '-'}',
            for (final id in sections) 'reveal $id ${sheet.reveal(id).toStringAsFixed(2)}',
          ];
          return Text(lines.join('\n'), style: style);
        },
      ),
    );
  }
}

class _NumberedTiles extends StatelessWidget {
  const _NumberedTiles({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    return SliverList.builder(
      itemCount: count,
      itemBuilder: (context, i) => ListTile(
        leading: CircleAvatar(child: Text('${i + 1}')),
        title: Text('Tile ${i + 1}'),
        onTap: () {},
      ),
    );
  }
}

/// A plain grid under the sheet, with a cross at the middle of what the sheet leaves
/// uncovered.
class _TestBody extends StatelessWidget {
  const _TestBody();

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return ColoredBox(
      color: scheme.surfaceDim,
      child: LayoutBuilder(
        builder: (context, constraints) {
          final size = constraints.biggest;
          return Stack(
            children: [
              Positioned.fill(child: CustomPaint(painter: _GridPainter(scheme.outlineVariant))),
              SheetBuilder(
                child: Icon(Icons.add, color: scheme.primary),
                builder: (context, sheet, child) {
                  final top = MediaQuery.paddingOf(context).top;
                  final clear = EdgeInsets.only(top: top) + sheet.obscured;
                  final center = clear.deflateRect(Offset.zero & size).center;
                  return Positioned(left: center.dx - 12, top: center.dy - 12, child: child!);
                },
              ),
            ],
          );
        },
      ),
    );
  }
}

class _GridPainter extends CustomPainter {
  _GridPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()..color = color;
    for (var x = 0.0; x < size.width; x += 48) {
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), paint);
    }
    for (var y = 0.0; y < size.height; y += 48) {
      canvas.drawLine(Offset(0, y), Offset(size.width, y), paint);
    }
  }

  @override
  bool shouldRepaint(_GridPainter oldDelegate) => color != oldDelegate.color;
}

/// A button riding on top of the sheet, fading out past halfway.
class _FloatingProbe extends StatelessWidget {
  const _FloatingProbe();

  @override
  Widget build(BuildContext context) {
    final padding = MediaQuery.paddingOf(context);
    return SheetBuilder(
      child: FloatingActionButton.small(
        heroTag: null,
        onPressed: () {},
        child: const Icon(Icons.adjust),
      ),
      builder: (context, sheet, child) {
        final docked = sheet.geometry.placement == SheetPlacement.docked;
        final height = sheet.geometry.size.height;
        final opacity = docked
            ? 1.0
            : (1 - (sheet.extent - height * 0.45) / (height * 0.1)).clamp(0.0, 1.0);
        return Stack(
          children: [
            Positioned(
              right: 16 + padding.right,
              bottom: 16 + (docked ? padding.bottom : sheet.extent),
              child: IgnorePointer(
                ignoring: opacity == 0,
                child: Opacity(opacity: opacity, child: child),
              ),
            ),
          ],
        );
      },
    );
  }
}
