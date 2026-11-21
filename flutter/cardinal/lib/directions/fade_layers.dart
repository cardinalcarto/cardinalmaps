/// One thing fading into another where it is, each keeping its own layout while both
/// show: the lists of two ways of traveling, say, whose rows aren't in the same places.
library;

import 'package:flutter/rendering.dart';
import 'package:material_ui/material_ui.dart';

/// Builds a layer of a [FadeLayers], which is [shown] from 0 to 1. The layer does the
/// fading itself, so it can fade what an ancestor's opacity wouldn't reach (the child
/// of a slot the sheet holds, which is painted apart from its ancestors). While it's
/// [leaving] it's on its way out, under the layer that took its place, and takes no
/// touches.
typedef LayerBuilder = Widget Function(
  Animation<double> shown, {
  required bool leaving,
});

/// Shows what [builder] builds. When [layer] changes, what was showing stays where
/// it was, as it was last built, and fades out over [duration] as the new layer fades
/// in over it: each is laid out on its own from the same top, and this is as tall as
/// the new one at once.
///
/// A layer that comes back while it's still fading out carries on from where it had
/// got to, with its state.
class FadeLayers extends StatefulWidget {
  const FadeLayers({
    super.key,
    required this.layer,
    required this.builder,
    this.duration = const Duration(milliseconds: 200),
  });

  /// Which layer [builder] builds.
  final Object layer;
  final LayerBuilder builder;
  final Duration duration;

  @override
  State<FadeLayers> createState() => _FadeLayersState();
}

class _Layer {
  _Layer(this.id, this.builder, this.shown);

  final Object id;
  LayerBuilder builder;
  final AnimationController shown;
}

class _FadeLayersState extends State<FadeLayers> with TickerProviderStateMixin {
  /// The layers, the one showing last, over those on their way out.
  late final List<_Layer> _layers = [_make(widget, shown: 1)];

  _Layer _make(FadeLayers widget, {required double shown}) {
    final controller = AnimationController(
      vsync: this,
      value: shown,
      duration: widget.duration,
    );
    final layer = _Layer(widget.layer, widget.builder, controller);
    controller.addStatusListener((status) {
      if (!status.isDismissed || identical(_layers.last, layer)) return;
      setState(() => _layers.remove(layer));
      controller.dispose();
    });
    return layer;
  }

  @override
  void didUpdateWidget(FadeLayers oldWidget) {
    super.didUpdateWidget(oldWidget);
    final current = _layers.last;
    if (widget.layer == current.id) {
      current.builder = widget.builder;
      return;
    }
    final back = _layers.where((layer) => layer.id == widget.layer).firstOrNull;
    final next = back ?? _make(widget, shown: 0);
    _layers
      ..remove(next)
      ..add(next);
    next
      ..builder = widget.builder
      ..shown.forward();
    current.shown.reverse();
  }

  @override
  void dispose() {
    for (final layer in _layers) {
      layer.shown.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => _Layers(
    children: [
      for (final layer in _layers)
        KeyedSubtree(
          key: ValueKey(layer.id),
          child: IgnorePointer(
            ignoring: !identical(layer, _layers.last),
            child: ExcludeSemantics(
              excluding: !identical(layer, _layers.last),
              child: layer.builder(
                layer.shown,
                leaving: !identical(layer, _layers.last),
              ),
            ),
          ),
        ),
    ],
  );
}

/// Its children one over another from the same top, each as wide as this and as tall
/// as it likes; this is as tall as the last, which is the only one that's hit.
class _Layers extends MultiChildRenderObjectWidget {
  const _Layers({required super.children});

  @override
  _RenderLayers createRenderObject(BuildContext context) => _RenderLayers();
}

class _LayerParentData extends ContainerBoxParentData<RenderBox> {}

class _RenderLayers extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _LayerParentData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _LayerParentData> {
  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _LayerParentData) {
      child.parentData = _LayerParentData();
    }
  }

  BoxConstraints _forChild(BoxConstraints constraints) =>
      BoxConstraints.tightFor(width: constraints.maxWidth);

  @override
  Size computeDryLayout(BoxConstraints constraints) => constraints.constrain(
    lastChild?.getDryLayout(_forChild(constraints)) ?? Size.zero,
  );

  @override
  double computeMinIntrinsicWidth(double height) =>
      lastChild?.getMinIntrinsicWidth(height) ?? 0;

  @override
  double computeMaxIntrinsicWidth(double height) =>
      lastChild?.getMaxIntrinsicWidth(height) ?? 0;

  @override
  double computeMinIntrinsicHeight(double width) =>
      lastChild?.getMinIntrinsicHeight(width) ?? 0;

  @override
  double computeMaxIntrinsicHeight(double width) =>
      lastChild?.getMaxIntrinsicHeight(width) ?? 0;

  @override
  void performLayout() {
    final forChild = _forChild(constraints);
    for (var child = firstChild; child != null; child = childAfter(child)) {
      child.layout(forChild, parentUsesSize: identical(child, lastChild));
    }
    size = constraints.constrain(lastChild?.size ?? Size.zero);
  }

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      lastChild?.hitTest(result, position: position) ?? false;
}
