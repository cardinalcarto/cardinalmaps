/// Finding the app's strings for a language without a widget tree, for code such as
/// navigation that runs with no `BuildContext` (and, later, under Android Auto and
/// CarPlay).
library;

import 'package:flutter/widgets.dart';

import 'app_localizations.dart';

/// The app's strings in [tag], a BCP-47 language tag such as `en-GB` or `pt-BR`, or null
/// when the app has no translation for its language.
///
/// The full tag is tried first, so a regional translation is used when there is one,
/// then just its language: `en-GB` gets the English strings. A language the app hasn't
/// been translated into gets null rather than another language's strings, because the
/// caller is speaking or showing them in [tag]: English read by a German voice sounds
/// broken, and silence is better.
AppLocalizations? localizationsFor(String tag) {
  final locale = _parse(tag);
  if (locale == null) return null;
  const supported = AppLocalizations.supportedLocales;
  final match =
      supported.where((l) => l == locale).firstOrNull ??
      supported
          .where(
            (l) =>
                l.languageCode == locale.languageCode &&
                l.scriptCode == null &&
                l.countryCode == null,
          )
          .firstOrNull;
  return match == null ? null : lookupAppLocalizations(match);
}

/// The app's strings in the device's language, for what's shown with no widget tree
/// to look them up in, such as navigation's Android notification; English when the
/// app has no translation for it, as the rest of the app falls back to.
AppLocalizations deviceLocalizations() =>
    localizationsFor(
      WidgetsBinding.instance.platformDispatcher.locale.toLanguageTag(),
    ) ??
    lookupAppLocalizations(const Locale('en'));

/// [tag] as a [Locale]: its language, then a four-letter script or a region, if it has
/// them. Null for an empty tag.
Locale? _parse(String tag) {
  final parts = tag.split(RegExp('[-_]'));
  final language = parts.first.toLowerCase();
  if (language.isEmpty) return null;
  String? script;
  String? country;
  for (final part in parts.skip(1)) {
    if (part.length == 4 && script == null && country == null) {
      script = '${part[0].toUpperCase()}${part.substring(1).toLowerCase()}';
    } else if ((part.length == 2 || part.length == 3) && country == null) {
      country = part.toUpperCase();
    }
  }
  return Locale.fromSubtags(
    languageCode: language,
    scriptCode: script,
    countryCode: country,
  );
}
