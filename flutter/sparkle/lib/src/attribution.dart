// The map's attribution: the credits its style's sources carry, as the OSMF's
// attribution guidelines have them shown on an interactive map
// (https://osmfoundation.org/wiki/Licence/Attribution_Guidelines). They're shown when
// the map opens and collapse to an ⓘ once the map is touched, and the ⓘ brings them
// back.

import 'package:flutter/foundation.dart' show immutable, listEquals;
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';

import 'sparkle_map.dart';

/// A run of an attribution's text, linking to [link] if it isn't null.
@immutable
class AttributionSpan {
  const AttributionSpan(this.text, {this.link});

  final String text;
  final Uri? link;

  @override
  bool operator ==(Object other) =>
      other is AttributionSpan && other.text == text && other.link == link;

  @override
  int get hashCode => Object.hash(text, link);

  @override
  String toString() => link == null ? 'Span($text)' : 'Span($text -> $link)';
}

/// The runs of an attribution as MapLibre styles write them: text with `<a href>` links
/// and HTML entities. Other tags are dropped, whitespace runs become single spaces, and
/// a link whose `href` isn't a URL is shown as plain text.
List<AttributionSpan> parseAttribution(String html) {
  final spans = <AttributionSpan>[];
  void add(String raw, Uri? link) {
    final text = _decodeEntities(raw.replaceAll(_tag, ''))
        .replaceAll(_space, ' ');
    if (text.isEmpty) return;
    if (spans.isNotEmpty && spans.last.link == link) {
      spans.last = AttributionSpan(spans.last.text + text, link: link);
    } else {
      spans.add(AttributionSpan(text, link: link));
    }
  }

  var at = 0;
  for (final match in _anchor.allMatches(html)) {
    add(html.substring(at, match.start), null);
    final href = _href.firstMatch(match[1]!);
    final link = href == null
        ? null
        : Uri.tryParse(_decodeEntities(href[1] ?? href[2] ?? '').trim());
    add(match[2]!, link != null && link.hasScheme ? link : null);
    at = match.end;
  }
  add(html.substring(at), null);
  if (spans.isEmpty) return spans;
  // Trimmed at the ends, as HTML would show it.
  spans.first = AttributionSpan(
    spans.first.text.trimLeft(),
    link: spans.first.link,
  );
  spans.last = AttributionSpan(
    spans.last.text.trimRight(),
    link: spans.last.link,
  );
  return spans.where((s) => s.text.isNotEmpty).toList();
}

final _anchor = RegExp(
  r'<a\b([^>]*)>(.*?)</a\s*>',
  caseSensitive: false,
  dotAll: true,
);
final _href = RegExp(
  r'''\bhref\s*=\s*(?:"([^"]*)"|'([^']*)')''',
  caseSensitive: false,
);
final _tag = RegExp(r'<[^>]*>');
final _space = RegExp(r'\s+');
final _entity = RegExp(r'&(#[0-9]+|#[xX][0-9a-fA-F]+|[a-zA-Z]+);');
const _entities = {
  'amp': '&',
  'lt': '<',
  'gt': '>',
  'quot': '"',
  'apos': "'",
  'nbsp': ' ',
  'copy': '©',
  'reg': '®',
  'trade': '™',
  'middot': '·',
  'ndash': '–',
  'mdash': '—',
};

String _decodeEntities(String text) => text.replaceAllMapped(_entity, (m) {
  final name = m[1]!;
  if (name.startsWith('#')) {
    final hex = name.length > 1 && (name[1] == 'x' || name[1] == 'X');
    final code = int.tryParse(
      name.substring(hex ? 2 : 1),
      radix: hex ? 16 : 10,
    );
    return code == null || code > 0x10ffff ? m[0]! : String.fromCharCode(code);
  }
  return _entities[name] ?? m[0]!;
});

/// The credits of [controller]'s map's style, for a corner of the map: shown when the
/// map opens, collapsed to an ⓘ once the map is touched, and shown again by tapping the
/// ⓘ or when the style's credits change. Nothing is shown while the style has none.
///
/// The ⓘ stays where it is as the credits open beside it, toward the end of the line,
/// and wrap upward from the bottom: place it in a bottom start corner. It needs a
/// bounded width to wrap in.
class SparkleAttribution extends StatefulWidget {
  const SparkleAttribution({
    super.key,
    required this.controller,
    this.onLink,
    this.background = const Color(0xd9ffffff),
    this.foreground = const Color(0xff202124),
    this.linkColor,
    this.textStyle,
    this.showLabel = 'Show map credits',
    this.hideLabel = 'Hide map credits',
  });

  final SparkleMapController controller;

  /// Called with a credit's link when it's tapped; without it, links are plain text.
  final void Function(Uri link)? onLink;

  /// The color of the rounded box behind it.
  final Color background;

  /// The color of the text and the ⓘ.
  final Color foreground;

  /// The color of links, underlined; [foreground] by default.
  final Color? linkColor;

  /// The style of the credits, over a 12 pixel [foreground] text.
  final TextStyle? textStyle;

  /// What the ⓘ says to a screen reader while the credits are hidden, and shown.
  final String showLabel;
  final String hideLabel;

  @override
  State<SparkleAttribution> createState() => _SparkleAttributionState();
}

class _SparkleAttributionState extends State<SparkleAttribution> {
  bool _open = true;
  List<String> _credits = const [];
  List<List<AttributionSpan>> _parsed = const [];

  /// The taps on links, by credit and span; made with the credits, so a build doesn't
  /// dispose of ones the text still holds.
  final Map<(int, int), TapGestureRecognizer> _recognizers = {};

  @override
  void initState() {
    super.initState();
    _listen(widget.controller);
    _read();
  }

  @override
  void didUpdateWidget(SparkleAttribution oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      _unlisten(oldWidget.controller);
      _listen(widget.controller);
      _read();
    }
  }

  @override
  void dispose() {
    _unlisten(widget.controller);
    _disposeRecognizers();
    super.dispose();
  }

  void _listen(SparkleMapController c) {
    c.attributions.addListener(_changed);
    c.touched.addListener(_touched);
  }

  void _unlisten(SparkleMapController c) {
    c.attributions.removeListener(_changed);
    c.touched.removeListener(_touched);
  }

  /// Reads the credits, returning whether they changed.
  bool _read() {
    final credits = widget.controller.attributions.value;
    if (listEquals(credits, _credits)) return false;
    _credits = credits;
    _parsed = [for (final c in credits) parseAttribution(c)];
    _disposeRecognizers();
    for (final (i, spans) in _parsed.indexed) {
      for (final (j, span) in spans.indexed) {
        if (span.link case final uri?) {
          _recognizers[(i, j)] = TapGestureRecognizer()
            ..onTap = () => widget.onLink?.call(uri);
        }
      }
    }
    return true;
  }

  void _changed() {
    if (!_read()) return;
    // New credits are shown, as the first ones were.
    setState(() => _open = true);
  }

  void _touched() {
    if (_open) setState(() => _open = false);
  }

  void _disposeRecognizers() {
    for (final r in _recognizers.values) {
      r.dispose();
    }
    _recognizers.clear();
  }

  InlineSpan _credit(int i, List<AttributionSpan> spans, TextStyle link) =>
      TextSpan(
        children: [
          for (final (j, span) in spans.indexed)
            if (_recognizers[(i, j)] case final tap? when widget.onLink != null)
              TextSpan(text: span.text, style: link, recognizer: tap)
            else
              TextSpan(text: span.text),
        ],
      );

  @override
  Widget build(BuildContext context) {
    if (_parsed.isEmpty) return const SizedBox.shrink();
    final text = DefaultTextStyle.of(context).style
        .merge(TextStyle(fontSize: 12, height: 1.25, color: widget.foreground))
        .merge(widget.textStyle);
    final link = TextStyle(
      color: widget.linkColor ?? text.color,
      decoration: TextDecoration.underline,
      decorationColor: widget.linkColor ?? text.color,
    );
    return DecoratedBox(
      decoration: BoxDecoration(
        color: widget.background,
        borderRadius: const BorderRadius.all(Radius.circular(12)),
      ),
      child: AnimatedSize(
        duration: const Duration(milliseconds: 200),
        curve: Curves.easeOutCubic,
        alignment: AlignmentDirectional.bottomStart,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            Semantics(
              button: true,
              label: _open ? widget.hideLabel : widget.showLabel,
              excludeSemantics: true,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () => setState(() => _open = !_open),
                child: Padding(
                  padding: const EdgeInsets.all(2),
                  child: CustomPaint(
                    size: const Size.square(20),
                    painter: _InfoIcon(widget.foreground),
                  ),
                ),
              ),
            ),
            if (_open)
              Flexible(
                child: Padding(
                  padding: const EdgeInsetsDirectional.only(
                    start: 2,
                    end: 8,
                    top: 4,
                    bottom: 4,
                  ),
                  child: Text.rich(
                    TextSpan(
                      style: text,
                      children: [
                        for (final (i, spans) in _parsed.indexed) ...[
                          if (i > 0) const TextSpan(text: ' · '),
                          _credit(i, spans, link),
                        ],
                      ],
                    ),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// ⓘ: a ring with an i in it.
class _InfoIcon extends CustomPainter {
  const _InfoIcon(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final s = size.shortestSide;
    final c = size.center(Offset.zero);
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = s * 0.08
      ..strokeCap = StrokeCap.round;
    canvas.drawCircle(c, s * 0.4, paint);
    canvas.drawLine(
      c + Offset(0, -s * 0.03),
      c + Offset(0, s * 0.2),
      paint..strokeWidth = s * 0.1,
    );
    canvas.drawCircle(
      c + Offset(0, -s * 0.19),
      s * 0.06,
      Paint()..color = color,
    );
  }

  @override
  bool shouldRepaint(_InfoIcon old) => old.color != color;
}
