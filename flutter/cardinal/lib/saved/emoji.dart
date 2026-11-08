import 'package:characters/characters.dart';

/// Whether [grapheme], one user-perceived character, is an emoji.
bool isEmoji(String grapheme) => _emoji.hasMatch(grapheme);

// Pictographs, flags, and keycaps such as 1️⃣.
final _emoji = RegExp(
  // ignore: valid_regexps, the lint doesn't know Unicode property escapes
  r'\p{Extended_Pictographic}|\p{Regional_Indicator}|⃣',
  unicode: true,
);

/// The last emoji in [text], or null if it has none.
String? lastEmoji(String text) {
  String? last;
  for (final grapheme in text.characters) {
    if (isEmoji(grapheme)) last = grapheme;
  }
  return last;
}
