import 'package:flutter_test/flutter_test.dart';
import 'package:sparkle/sparkle.dart';

void main() {
  test('plain text, with entities', () {
    expect(parseAttribution('  © OpenStreetMap &amp; friends &copy; &#169; '), [
      const AttributionSpan('© OpenStreetMap & friends © ©'),
    ]);
  });

  test('links among text', () {
    expect(
      parseAttribution(
        '<a href="https://openmaptiles.org/">&copy; OpenMapTiles</a> '
        "<A HREF='https://www.openstreetmap.org/copyright' target=_blank>"
        '&copy; OpenStreetMap contributors</A>',
      ),
      [
        AttributionSpan(
          '© OpenMapTiles',
          link: Uri.parse('https://openmaptiles.org/'),
        ),
        const AttributionSpan(' '),
        AttributionSpan(
          '© OpenStreetMap contributors',
          link: Uri.parse('https://www.openstreetmap.org/copyright'),
        ),
      ],
    );
  });

  test('other tags dropped, whitespace collapsed', () {
    expect(parseAttribution('<b>Map</b>\n  data <span>here</span>'), [
      const AttributionSpan('Map data here'),
    ]);
  });

  test('a link without a URL is plain text', () {
    expect(parseAttribution('a <a href="nowhere">b</a> <a>c</a>'), [
      const AttributionSpan('a b c'),
    ]);
  });

  test('unknown entities are left alone', () {
    expect(parseAttribution('x &bogus; &#xZZ;'), [
      const AttributionSpan('x &bogus; &#xZZ;'),
    ]);
  });
}
