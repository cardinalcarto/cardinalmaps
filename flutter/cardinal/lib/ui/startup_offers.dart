/// What the app asks at startup, one thing at a time: an offer that comes later waits
/// for the ones before it to be answered, so it never opens under them.
library;

import 'dart:async';

import 'package:flutter/widgets.dart';

/// The startup offers under way. An offer that goes first [hold]s the ones after it
/// until it's answered; those wait for [answered].
class StartupOffers {
  final _held = <Future<void>>[];

  /// Holds the offers that wait until [answer] completes, however it ends.
  void hold(Future<void> answer) =>
      _held.add(answer.then<void>((_) {}, onError: (Object _) {}));

  /// Waits only for the offers already held, not ones registered later.
  Future<void> answeredSoFar() async => Future.wait(_held.toList());

  /// Completes once every offer held so far is answered, including any held while
  /// waiting.
  Future<void> answered() async {
    var waited = 0;
    while (waited < _held.length) {
      final held = _held.sublist(waited);
      waited = _held.length;
      await Future.wait(held);
    }
  }
}

/// Gives its subtree [offers].
class StartupOffersScope extends InheritedWidget {
  const StartupOffersScope({
    super.key,
    required this.offers,
    required super.child,
  });

  final StartupOffers offers;

  /// The offers above [context], or null if there are none to wait for or hold.
  static StartupOffers? maybeOf(BuildContext context) =>
      context.getInheritedWidgetOfExactType<StartupOffersScope>()?.offers;

  @override
  bool updateShouldNotify(StartupOffersScope oldWidget) =>
      offers != oldWidget.offers;
}
