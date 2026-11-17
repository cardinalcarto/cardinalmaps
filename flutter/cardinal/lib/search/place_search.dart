import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import '../geocoder/geocoder.dart';
import '../store/store.dart';

/// Search as you type: results for the latest text, fetched once typing pauses.
///
/// Requests in flight keep going when the text changes, so results can be a few
/// characters behind while typing; an answer never replaces a newer one's.
///
/// Search goes to [offline], or to [online] while the [searchOnline] setting says so;
/// when that changes, the text is searched again where it now goes.
///
/// When [offline] finds nothing, the [onlineFallback] setting says what's next:
///
/// | Fallback | Then |
/// |---|---|
/// | not chosen | [offersOnline]: the results ask whether to search online |
/// | [OnlineFallback.always] | the same text is searched [online], and its results show |
/// | [OnlineFallback.never] | no results |
///
/// [searchOnlineOnce] searches the text online once; the next text goes offline again.
class PlaceSearch extends ChangeNotifier {
  PlaceSearch({
    required this.offline,
    required this.online,
    SettingsStore? settings,
    this.lang,
    this.debounce = const Duration(milliseconds: 300),
  }) : _settings = settings {
    _watchOnline = settings?.watch(searchOnline).listen((online) {
      if (online == _online) return;
      _online = online;
      _searchAgain();
    });
    _watchFallback = settings?.watch(onlineFallback).listen((fallback) {
      if (fallback == _fallback) return;
      _fallback = fallback;
      notifyListeners();
    });
  }

  final SettingsStore? _settings;
  StreamSubscription<OnlineFallback?>? _watchFallback;

  /// What search does when [offline] finds nothing: the [onlineFallback] setting.
  OnlineFallback? get fallback => _fallback;
  OnlineFallback? _fallback;

  /// Whether [results] came from [online].
  bool get resultsOnline => _resultsOnline;
  bool _resultsOnline = false;

  /// Whether to ask if [text] should be searched online: the device found nothing for
  /// it, and no [fallback] is chosen.
  bool get offersOnline =>
      !_online &&
      _fallback == null &&
      !_loading &&
      _error == null &&
      !_resultsOnline &&
      _results.isEmpty &&
      _text.trim().isNotEmpty;

  /// Searches [text] online, now, once: the next text goes where search goes.
  void searchOnlineOnce() {
    if (_text.trim().isEmpty) return;
    _timer?.cancel();
    _loading = true;
    notifyListeners();
    unawaited(_search(_text, forceOnline: true));
  }

  /// Chooses what search does when the device finds nothing, and stores it. Choosing
  /// [OnlineFallback.always] searches [text] online now too.
  void chooseFallback(OnlineFallback fallback) {
    _fallback = fallback;
    unawaited(_settings?.set(onlineFallback, fallback));
    if (fallback == OnlineFallback.always) {
      searchOnlineOnce();
    } else {
      notifyListeners();
    }
  }

  final Geocoder offline;
  final Geocoder online;
  StreamSubscription<bool>? _watchOnline;

  /// Whether search goes [online]: the [searchOnline] setting, offline until it's read.
  bool get searchesOnline => _online;
  bool _online = false;

  /// The language of queries and results, as a BCP 47 tag.
  final String? lang;
  final Duration debounce;

  /// Results near here rank higher.
  LngLat? focus;

  /// What the map shows, which says how much nearness counts.
  Viewport? viewport;

  /// The text the results are for, or will be once they arrive.
  String get text => _text;
  String _text = '';

  /// Results for [text], most relevant first. Kept from the previous text while new ones
  /// load, so the list doesn't flash empty.
  List<Place> get results => _results;
  List<Place> _results = const [];

  /// Whether results for [text] are still to come.
  bool get loading => _loading;
  bool _loading = false;

  /// Why the last search failed, if it did.
  Object? get error => _error;
  Object? _error;

  Timer? _timer;

  /// How many requests have been sent; each is numbered by the count when it was sent.
  int _sent = 0;

  /// The number of the newest request whose answer is showing (or that clearing the
  /// text overtook): answers to older ones are dropped.
  int _shown = 0;

  /// Completes when this is disposed, cancelling whatever is in flight.
  final _closed = Completer<void>();

  /// The text changed.
  void update(String text) {
    if (text == _text) return;
    _text = text;
    _timer?.cancel();
    if (text.trim().isEmpty) {
      // Answers still on their way are for text that's gone.
      _shown = _sent;
      _results = const [];
      _resultsOnline = false;
      _loading = false;
      _error = null;
    } else {
      _loading = true;
      _timer = Timer(debounce, () => _search(text));
    }
    notifyListeners();
  }

  /// Searches the text again, if there is any, as if it had just been typed.
  void _searchAgain() {
    if (_text.trim().isEmpty) return;
    _timer?.cancel();
    _loading = true;
    _timer = Timer(debounce, () => _search(_text));
    notifyListeners();
  }

  /// Search for [text], leaving earlier requests running: for a fast typist, their
  /// answers are the only ones that arrive while typing, a few characters behind.
  ///
  /// The device's search falls back to [online] if it finds nothing and [fallback] is
  /// [OnlineFallback.always]; [forceOnline] searches online whatever the setting.
  Future<void> _search(String text, {bool forceOnline = false}) async {
    final number = ++_sent;
    List<Place>? results;
    Object? error;
    var fromOnline = _online || forceOnline;
    Future<List<Place>> find(Geocoder geocoder) => geocoder.search(
      text,
      focus: focus,
      viewport: viewport,
      lang: lang,
      prefix: true,
      typos: true,
      abort: _closed.future,
    );
    try {
      results = await find(fromOnline ? online : offline);
      if (results.isEmpty &&
          !fromOnline &&
          _fallback == OnlineFallback.always) {
        fromOnline = true;
        results = await find(online);
      }
    } on http.RequestAbortedException {
      return;
    } on Object catch (e) {
      error = e;
    }
    if (_closed.isCompleted || number <= _shown) return;
    _shown = number;
    if (results != null) _results = results;
    _resultsOnline = fromOnline;
    _error = error;
    _loading = _sent > _shown || (_timer?.isActive ?? false);
    notifyListeners();
  }

  @override
  void dispose() {
    _watchOnline?.cancel();
    _watchFallback?.cancel();
    _timer?.cancel();
    _closed.complete();
    super.dispose();
  }
}
