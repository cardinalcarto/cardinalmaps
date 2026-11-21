/// A draggable sheet over a body (the map), with any number of anchors, content that
/// animates with how much of it is showing, and pages that swap within the one sheet.
///
/// - [SheetScaffold] lays the sheet over its body and hosts the sheet's navigator.
/// - [SheetPage] is a page within that navigator; its child is usually a [SheetContent].
/// - [SheetContent] declares where the sheet can rest ([SheetAnchor]s) and what it shows.
/// - [SheetSection] marks a span of the content that anchors and animations refer to.
/// - [SheetSlot] marks a part of the content that can be held in view as the rest of
///   it passes behind ([SheetContent.held]).
/// - [SheetBuilder] rebuilds as the sheet moves, with a [SheetView] of where it is.
library;

import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show lerpDouble;

import 'package:collection/collection.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/physics.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

part 'src/anchor.dart';
part 'src/content.dart';
part 'src/controller.dart';
part 'src/geometry.dart';
part 'src/held.dart';
part 'src/page.dart';
part 'src/physics.dart';
part 'src/scaffold.dart';
part 'src/scroll.dart';
