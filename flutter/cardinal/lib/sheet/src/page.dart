part of '../sheet.dart';

/// A page of the sheet's navigator, usually showing a [SheetContent].
///
/// Pages swap within the one sheet: the content fades from one page to the next while
/// the sheet moves between their anchors, and an Android back gesture scrubs
/// both back towards the page below. A page restored on top of others opens the same
/// way. Give pages a [restorationId] (go_router's `state.pageKey.value`) to restore them.
class SheetPage<T> extends Page<T> {
  const SheetPage({
    required this.child,
    super.key,
    super.name,
    super.arguments,
    super.restorationId,
  });

  final Widget child;

  @override
  Route<T> createRoute(BuildContext context) => _SheetPageRoute<T>(this);
}

class _SheetPageRoute<T> extends PageRoute<T> {
  _SheetPageRoute(SheetPage<T> page) : super(settings: page);

  SheetPage<T> get _page => settings as SheetPage<T>;

  @override
  Duration get transitionDuration => Durations.medium2;

  @override
  Duration get reverseTransitionDuration => Durations.medium2;

  /// Starts at once whichever way it's going, and slows into place.
  static const Curve _curve = Curves.easeOutCubic;

  late final CurvedAnimation _arriving;
  late final CurvedAnimation _covered;

  @override
  void install() {
    super.install();
    _arriving = CurvedAnimation(parent: animation!, curve: _curve, reverseCurve: _curve.flipped);
    _covered = CurvedAnimation(parent: secondaryAnimation!, curve: _curve, reverseCurve: _curve.flipped);
  }

  @override
  void dispose() {
    _arriving.dispose();
    _covered.dispose();
    super.dispose();
  }

  /// How far [curved] has got: eased, unless a gesture is driving it, in which case it
  /// follows the gesture. Only ever switches between the two at either end of a transition,
  /// where both agree.
  double _progress(CurvedAnimation curved) {
    if (navigator?.userGestureInProgress ?? false) return curved.parent.value.clamp(0.0, 1.0);
    return curved.value;
  }

  /// How far this page has come in over the page below.
  double get _arrival => _progress(_arriving);

  /// How far a page has come in over this one.
  double get _cover => _progress(_covered);

  @override
  Color? get barrierColor => null;

  @override
  String? get barrierLabel => null;

  @override
  bool get maintainState => true;

  @override
  bool canTransitionTo(TransitionRoute<dynamic> nextRoute) => nextRoute is _SheetPageRoute;

  /// A page added without being pushed (restored, or under a deep link) opens as if it
  /// had been, if it's on top: the sheet moves up from the page below as the content
  /// fades in, and whatever follows the route's arrival (or departure) sees the same
  /// as after a push. The bottom page just shows.
  @override
  void didAdd() {
    super.didAdd();
    if (isCurrent && !isFirst) controller!.forward(from: 0);
  }

  /// Whether this route has been popped. Until the navigator catches up it may still be
  /// current, but a back gesture must leave its transition alone from then on.
  bool _popped = false;

  @override
  bool didPop(T? result) {
    _popped = true;
    return super.didPop(result);
  }

  @override
  bool canTransitionFrom(TransitionRoute<dynamic> previousRoute) =>
      previousRoute is _SheetPageRoute;

  @override
  Widget buildPage(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
  ) => _page.child;

  @override
  Widget buildTransitions(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
    Widget child,
  ) {
    return _SheetBackGesture(
      route: this,
      child: _SheetFade(
        route: this,
        child: child,
      ),
    );
  }

  /// Follows a back gesture at [progress] (1 showing this page, 0 showing the one
  /// below), taking over from the route's own animation.
  void _startBackGesture(double progress) {
    navigator!.didStartUserGesture();
    controller!.value = progress;
  }

  void _updateBackGesture(double progress) {
    if (isCurrent && !_popped) controller!.value = progress;
  }

  /// Finishes a back gesture from wherever it was let go, popping if [pop].
  ///
  /// The route's own handling of predictive back starts the pop over from the top,
  /// which jumps the content and the sheet back before they leave.
  ///
  /// If the route was popped some other way during the gesture (by a close button, say),
  /// its pop carries on as it is.
  void _endBackGesture({required bool pop}) {
    final navigator = this.navigator!;
    final controller = this.controller!;
    if (_popped) {
      // Popped already: neither again nor back into view.
    } else if (pop) {
      navigator.pop();
      if (controller.isAnimating) {
        controller.animateBack(
          0,
          duration: reverseTransitionDuration * controller.value,
          curve: Curves.easeOutCubic,
        );
      }
    } else if (!controller.isCompleted) {
      controller.animateTo(
        1,
        duration: transitionDuration * (1 - controller.value),
        curve: Curves.easeOutCubic,
      );
    }
    // The transition follows the gesture linearly until it's over.
    if (controller.isAnimating) {
      void stop(AnimationStatus status) {
        if (status.isAnimating) return;
        controller.removeStatusListener(stop);
        navigator.didStopUserGesture();
      }

      controller.addStatusListener(stop);
    } else {
      navigator.didStopUserGesture();
    }
  }
}

/// Fades a page's content out as one is pushed over it, then the new page's in, so the
/// two are never both legible; popping runs the same in reverse.
class _SheetFade extends StatelessWidget {
  const _SheetFade({required this.route, required this.child});

  final _SheetPageRoute<dynamic> route;
  final Widget child;

  /// How far through a transition the page below has gone and the new page starts.
  static const double _handover = 0.35;

  @override
  Widget build(BuildContext context) {
    final color = _SheetScope.of(context).color;
    return AnimatedBuilder(
      animation: Listenable.merge([route.animation, route.secondaryAnimation]),
      // Opaque, to cover the page below once it's in, and a Material of its own for ink
      // to show on.
      child: Material(color: color, child: child),
      builder: (context, child) {
        final fadedIn = ((route._arrival - _handover) / (1 - _handover)).clamp(0.0, 1.0);
        final fadedOut = (route._cover / _handover).clamp(0.0, 1.0);
        return Opacity(opacity: fadedIn * (1 - fadedOut), child: child);
      },
    );
  }
}

/// Hands Android back gestures to its route while that route is current, both in the
/// sheet's navigator and above it, so the gesture scrubs the route's transition (and so
/// the sheet's move between the two pages' anchors).
class _SheetBackGesture extends StatefulWidget {
  const _SheetBackGesture({required this.route, required this.child});

  final _SheetPageRoute<dynamic> route;
  final Widget child;

  @override
  State<_SheetBackGesture> createState() => _SheetBackGestureState();
}

class _SheetBackGestureState extends State<_SheetBackGesture> with WidgetsBindingObserver {
  bool _gesturing = false;

  bool get _enabled {
    final route = widget.route;
    if (!route.isCurrent || route._popped || !route.popGestureEnabled) return false;
    final scope = context.getInheritedWidgetOfExactType<_SheetScope>();
    return scope?.state._isRouteCurrent ?? true;
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  bool handleStartBackGesture(PredictiveBackEvent backEvent) {
    if (backEvent.isButtonEvent || !_enabled) return false;
    _gesturing = true;
    widget.route._startBackGesture(1 - backEvent.progress);
    return true;
  }

  @override
  void handleUpdateBackGestureProgress(PredictiveBackEvent backEvent) {
    if (_gesturing) widget.route._updateBackGesture(1 - backEvent.progress);
  }

  @override
  void handleCancelBackGesture() {
    if (!_gesturing) return;
    _gesturing = false;
    widget.route._endBackGesture(pop: false);
  }

  @override
  void handleCommitBackGesture() {
    if (!_gesturing) return;
    _gesturing = false;
    widget.route._endBackGesture(pop: true);
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
