/// What the sheet shows at each step of downloading an area (see `download_page.dart`):
/// the bar while drawing, measuring, a failure to measure, the size page, and the
/// transit page.
library;

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoActivityIndicator,
        CupertinoIcons,
        CupertinoTextField,
        OverlayVisibilityMode;
import 'package:material_ui/material_ui.dart';

import '../ui/adaptive.dart';
import 'downloads/plan.dart';
import 'drawing/lasso.dart';
import 'format.dart';

/// Room left around the sheet's content, as the place card has.
const _padding = EdgeInsets.fromLTRB(16, 4, 16, 16);

/// While drawing: the hint, or why the last lasso wasn't taken, and Cancel.
class DrawingBar extends StatelessWidget {
  const DrawingBar({super.key, required this.problem, required this.onCancel});

  /// Why the last lasso wasn't taken, if it wasn't.
  final LassoProblem? problem;
  final VoidCallback onCancel;

  static String _message(LassoProblem? problem) => switch (problem) {
    null => 'Draw around the area you want',
    LassoProblem.tooSmall =>
      "That's too small to download. Draw around a larger area.",
    LassoProblem.crossesItself =>
      'The outline crosses itself. Draw one loop around the area.',
    LassoProblem.offMap => 'Keep the outline on the map.',
  };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    return _Bar(
      leading: Icon(
        problem == null
            ? _icon(context, Icons.gesture, CupertinoIcons.hand_draw)
            : _icon(context, Icons.info_outline, CupertinoIcons.info),
        color: problem == null ? colors.primary : colors.onSurfaceVariant,
      ),
      text: AnimatedSwitcher(
        duration: const Duration(milliseconds: 200),
        layoutBuilder: (current, previous) => Stack(
          alignment: AlignmentDirectional.centerStart,
          children: [...previous, ?current],
        ),
        child: Text(
          _message(problem),
          key: ValueKey(problem),
          style: theme.textTheme.bodyMedium?.copyWith(
            color: problem == null ? colors.onSurfaceVariant : colors.onSurface,
          ),
        ),
      ),
      action: AppButton(
        kind: AppButtonKind.plain,
        label: 'Cancel',
        onPressed: onCancel,
      ),
    );
  }
}

/// While the plan is read: a spinner, and Redraw.
class MeasuringBar extends StatelessWidget {
  const MeasuringBar({super.key, required this.onRedraw});

  final VoidCallback onRedraw;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return _Bar(
      leading: isCupertinoOf(context)
          ? const CupertinoActivityIndicator()
          : const SizedBox.square(
              dimension: 20,
              child: CircularProgressIndicator(strokeWidth: 2.5),
            ),
      text: Text(
        'Measuring this area…',
        style: theme.textTheme.bodyMedium?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
      action: AppButton(
        kind: AppButtonKind.plain,
        label: 'Redraw',
        onPressed: onRedraw,
      ),
    );
  }
}

/// A row as tall as a button: [leading], [text] and an [action] at the end.
class _Bar extends StatelessWidget {
  const _Bar({required this.leading, required this.text, required this.action});

  final Widget leading;
  final Widget text;
  final Widget action;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsetsDirectional.fromSTEB(20, 0, 8, 12),
    child: ConstrainedBox(
      constraints: const BoxConstraints(minHeight: 48),
      child: Row(
        children: [
          SizedBox(width: 24, child: Center(child: leading)),
          const SizedBox(width: 12),
          Expanded(child: text),
          const SizedBox(width: 8),
          action,
        ],
      ),
    ),
  );
}

/// Measuring failed: why, with Redraw and Try again.
class MeasureFailed extends StatelessWidget {
  const MeasureFailed({
    super.key,
    required this.offline,
    required this.onRedraw,
    required this.onRetry,
  });

  /// Whether the download server couldn't be reached at all, rather than answering
  /// wrong.
  final bool offline;
  final VoidCallback onRedraw;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: _padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text("Couldn't measure this area", style: theme.textTheme.titleLarge),
          const SizedBox(height: 8),
          Text(
            offline
                ? "The download server can't be reached. Check your connection, "
                      'then try again.'
                : "The download server didn't answer as expected. Try again in "
                      'a little while.',
            style: theme.textTheme.bodyLarge?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: 20),
          _Buttons(
            start: AppButton(
              kind: AppButtonKind.tinted,
              label: 'Redraw',
              onPressed: onRedraw,
            ),
            end: AppButton(label: 'Try again', onPressed: onRetry),
          ),
        ],
      ),
    );
  }
}

/// Page 1 of the confirmation: how much the area's maps and directions need, the free
/// space, and the area's name.
class SizeStep extends StatelessWidget {
  const SizeStep({
    super.key,
    required this.bytes,
    required this.free,
    required this.fits,
    required this.hasTransit,
    required this.name,
    required this.busy,
    required this.onRedraw,
    required this.onNext,
    required this.onDownload,
  });

  /// The map and routing bytes to download.
  final int bytes;

  /// The bytes free, if known.
  final int? free;

  /// Whether the download fits in [free], with room to spare.
  final bool fits;

  /// Whether there are transit feeds to choose next: the page then goes on to them
  /// rather than downloading.
  final bool hasTransit;
  final TextEditingController name;

  /// Whether the download is starting: the buttons wait.
  final bool busy;
  final VoidCallback onRedraw;
  final VoidCallback onNext;
  final VoidCallback onDownload;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final body = theme.textTheme.bodyLarge?.copyWith(
      color: colors.onSurfaceVariant,
    );
    final strong = TextStyle(
      color: colors.onSurface,
      fontWeight: FontWeight.w600,
    );
    final size = formatBytes(bytes);
    final free = this.free;
    return Padding(
      padding: _padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text('Download this area?', style: theme.textTheme.headlineSmall),
          const SizedBox(height: 8),
          // Inside areas already downloaded: nothing new to fetch.
          if (fits && bytes == 0)
            Text(
              'Maps and directions for this area are already downloaded.',
              style: body,
            )
          else if (fits)
            Text.rich(
              TextSpan(
                children: [
                  const TextSpan(
                    text: 'Maps and directions for this area need about ',
                  ),
                  TextSpan(text: size, style: strong),
                  const TextSpan(text: '.'),
                  if (free != null) ...[
                    const TextSpan(text: ' You have '),
                    TextSpan(text: formatBytes(free), style: strong),
                    const TextSpan(text: ' free.'),
                  ],
                ],
              ),
              style: body,
            )
          else
            _NotEnoughSpace(
              size: size,
              free: free ?? 0,
              style: body,
              strong: strong,
            ),
          const SizedBox(height: 16),
          _NameField(controller: name),
          const SizedBox(height: 20),
          _Buttons(
            start: AppButton(
              kind: AppButtonKind.tinted,
              label: 'Redraw',
              onPressed: busy ? null : onRedraw,
            ),
            end: AppButton(
              label: hasTransit ? 'Next' : 'Download',
              onPressed: busy || !fits
                  ? null
                  : (hasTransit ? onNext : onDownload),
            ),
          ),
        ],
      ),
    );
  }
}

/// Page 2 of the confirmation: the transit feeds to download, the total with them,
/// and Download. The list scrolls within [maxListHeight], so the area stays in view
/// above; the total and the buttons stay below it.
class TransitStep extends StatelessWidget {
  const TransitStep({
    super.key,
    required this.feeds,
    required this.picked,
    required this.total,
    required this.free,
    required this.fits,
    required this.busy,
    required this.onToggle,
    required this.onBack,
    required this.onDownload,
  });

  /// The feeds to choose from, in the order they're listed.
  final List<CandidateFeed> feeds;

  /// The IDs of those chosen.
  final Set<String> picked;

  /// The bytes to download with [picked].
  final int total;

  /// The bytes free, if known.
  final int? free;

  /// Whether [total] fits in [free], with room to spare.
  final bool fits;

  /// Whether the download is starting: the rows and buttons wait.
  final bool busy;
  final ValueChanged<String> onToggle;
  final VoidCallback onBack;
  final VoidCallback onDownload;

  /// The list's height, at most: about a third of the screen.
  static double maxListHeight(BuildContext context) =>
      MediaQuery.sizeOf(context).height * 0.34;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: EdgeInsets.fromLTRB(
            _padding.left,
            _padding.top,
            _padding.right,
            0,
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text('Transit', style: theme.textTheme.headlineSmall),
              const SizedBox(height: 8),
              Text(
                'Choose the agencies you ride.',
                style: theme.textTheme.bodyLarge?.copyWith(
                  color: colors.onSurfaceVariant,
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 8),
        ConstrainedBox(
          constraints: BoxConstraints(maxHeight: maxListHeight(context)),
          child: _FeedList(
            feeds: feeds,
            picked: picked,
            onToggle: busy ? null : onToggle,
          ),
        ),
        Padding(
          padding: EdgeInsets.fromLTRB(
            _padding.left,
            12,
            _padding.right,
            _padding.bottom,
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _Total(total: total, free: free, fits: fits),
              const SizedBox(height: 20),
              _Buttons(
                start: AppButton(
                  kind: AppButtonKind.tinted,
                  label: 'Back',
                  onPressed: busy ? null : onBack,
                ),
                end: AppButton(
                  label: 'Download',
                  onPressed: busy || !fits ? null : onDownload,
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// The feeds, one row each, scrolling when there are more than fit. A hairline at an
/// edge says there's more past it.
class _FeedList extends StatefulWidget {
  const _FeedList({
    required this.feeds,
    required this.picked,
    required this.onToggle,
  });

  final List<CandidateFeed> feeds;
  final Set<String> picked;

  /// Null while the rows wait.
  final ValueChanged<String>? onToggle;

  @override
  State<_FeedList> createState() => _FeedListState();
}

class _FeedListState extends State<_FeedList> {
  bool _above = false;
  bool _below = false;

  bool _onMetrics(ScrollMetrics metrics) {
    final above = metrics.extentBefore > 0.5;
    final below = metrics.extentAfter > 0.5;
    if (above != _above || below != _below) {
      setState(() {
        _above = above;
        _below = below;
      });
    }
    return false;
  }

  @override
  Widget build(BuildContext context) {
    final cupertino = isCupertinoOf(context);
    final rows = [
      for (final feed in widget.feeds)
        _FeedRow(
          key: ValueKey(feed.id),
          feed: feed,
          picked: widget.picked.contains(feed.id),
          onToggle: widget.onToggle,
        ),
    ];
    final list = ListView(
      shrinkWrap: true,
      padding: EdgeInsets.zero,
      children: cupertino ? [AppListSection(children: rows)] : rows,
    );
    return Stack(
      children: [
        NotificationListener<ScrollMetricsNotification>(
          onNotification: (n) => _onMetrics(n.metrics),
          child: NotificationListener<ScrollUpdateNotification>(
            onNotification: (n) => _onMetrics(n.metrics),
            child: list,
          ),
        ),
        Positioned(top: 0, left: 0, right: 0, child: _Edge(shown: _above)),
        Positioned(bottom: 0, left: 0, right: 0, child: _Edge(shown: _below)),
      ],
    );
  }
}

/// A hairline across the list's edge, faded in while rows are past it.
class _Edge extends StatelessWidget {
  const _Edge({required this.shown});

  final bool shown;

  @override
  Widget build(BuildContext context) => IgnorePointer(
    child: AnimatedOpacity(
      opacity: shown ? 1 : 0,
      duration: const Duration(milliseconds: 150),
      child: Divider(
        height: 1,
        thickness: 1,
        color: Theme.of(context).colorScheme.outlineVariant,
      ),
    ),
  );
}

/// A feed: whether it's chosen, its name, its agencies if it has several, and its
/// size. All of it toggles it.
class _FeedRow extends StatelessWidget {
  const _FeedRow({
    super.key,
    required this.feed,
    required this.picked,
    required this.onToggle,
  });

  final CandidateFeed feed;
  final bool picked;
  final ValueChanged<String>? onToggle;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final title = Text(
      feedTitle(feed),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
    );
    final subtitle = feed.agencies.length > 1
        ? AgencyLine(
            agencies: feed.agencies,
            style: theme.textTheme.bodyMedium?.copyWith(
              color: colors.onSurfaceVariant,
            ),
          )
        : null;
    final size = Text(
      formatBytes(feed.fileBytes),
      style: theme.textTheme.bodyMedium?.copyWith(
        color: colors.onSurfaceVariant,
        fontFeatures: const [FontFeature.tabularFigures()],
      ),
    );
    final toggle = onToggle == null ? null : () => onToggle!(feed.id);
    if (isCupertinoOf(context)) {
      return Semantics(
        checked: picked,
        child: AppListTile(
          // iOS's selection circle.
          leading: Icon(
            picked
                ? CupertinoIcons.checkmark_circle_fill
                : CupertinoIcons.circle,
            color: picked ? colors.primary : colors.outline,
          ),
          title: title,
          subtitle: subtitle,
          trailing: size,
          onTap: toggle,
        ),
      );
    }
    return CheckboxListTile(
      value: picked,
      onChanged: toggle == null ? null : (_) => toggle(),
      controlAffinity: ListTileControlAffinity.leading,
      visualDensity: VisualDensity.compact,
      // The box lines up with the page's text; the size with its end.
      contentPadding: const EdgeInsetsDirectional.only(start: 5, end: 16),
      title: title,
      subtitle: subtitle,
      secondary: size,
    );
  }
}

/// What a feed is called in its row: its agency's name if it has one agency, else its
/// name as published.
String feedTitle(CandidateFeed feed) =>
    feed.agencies.length == 1 ? feed.agencies.single : feed.displayName;

/// [agencies], comma-separated on one line, as many as fit and "+N" for the rest.
class AgencyLine extends StatelessWidget {
  const AgencyLine({super.key, required this.agencies, this.style});

  final List<String> agencies;
  final TextStyle? style;

  /// [agencies] up to [shown], and how many more there are.
  static String text(List<String> agencies, int shown) {
    final names = agencies.take(shown).join(', ');
    final rest = agencies.length - shown;
    return rest > 0 ? '$names +$rest' : names;
  }

  @override
  Widget build(BuildContext context) {
    final style = DefaultTextStyle.of(context).style.merge(this.style);
    final scaler = MediaQuery.textScalerOf(context);
    final direction = Directionality.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        bool fits(String text) {
          final painter = TextPainter(
            text: TextSpan(text: text, style: style),
            textDirection: direction,
            textScaler: scaler,
            maxLines: 1,
          )..layout(maxWidth: constraints.maxWidth);
          final fits = !painter.didExceedMaxLines;
          painter.dispose();
          return fits;
        }

        for (var shown = agencies.length; shown > 1; shown--) {
          final line = text(agencies, shown);
          if (fits(line)) return Text(line, style: style, maxLines: 1);
        }
        // Even the first alone doesn't fit: it's cut short, with the count after.
        final rest = agencies.length - 1;
        return Row(
          children: [
            Flexible(
              child: Text(
                agencies.first,
                style: style,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            if (rest > 0) Text(' +$rest', style: style, maxLines: 1),
          ],
        );
      },
    );
  }
}

/// The total to download and the free space, or, if it doesn't fit, that there isn't
/// enough space, as page 1 says it. The total runs to its new value as feeds are
/// chosen; going from one line to the other, they cross-fade at their new values.
class _Total extends StatelessWidget {
  const _Total({required this.total, required this.free, required this.fits});

  final int total;
  final int? free;
  final bool fits;

  static const _duration = Duration(milliseconds: 300);

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final body = theme.textTheme.bodyLarge?.copyWith(
      color: colors.onSurfaceVariant,
    );
    final strong = TextStyle(
      color: colors.onSurface,
      fontWeight: FontWeight.w600,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final free = this.free;
    return AnimatedSwitcher(
      duration: _duration,
      layoutBuilder: (current, previous) => Stack(
        alignment: AlignmentDirectional.topStart,
        children: [...previous, ?current],
      ),
      // Each line starts at the total it's shown with, and runs on from there.
      child: TweenAnimationBuilder<double>(
        key: ValueKey(fits),
        tween: Tween(end: total.toDouble()),
        duration: _duration,
        curve: Curves.easeOutCubic,
        builder: (context, value, _) {
          final size = formatBytes(value.round());
          if (fits) {
            return Text.rich(
              TextSpan(
                children: [
                  const TextSpan(text: 'Total '),
                  TextSpan(text: size, style: strong),
                  if (free != null) ...[
                    const TextSpan(text: ' · '),
                    TextSpan(text: formatBytes(free), style: strong),
                    const TextSpan(text: ' free'),
                  ],
                ],
              ),
              style: body,
            );
          }
          return _NotEnoughSpace(
            size: size,
            free: free ?? 0,
            style: body,
            strong: strong,
          );
        },
      ),
    );
  }
}

/// That [size] doesn't fit in [free], with a warning sign before it.
class _NotEnoughSpace extends StatelessWidget {
  const _NotEnoughSpace({
    required this.size,
    required this.free,
    required this.style,
    required this.strong,
  });

  final String size;
  final int free;
  final TextStyle? style;
  final TextStyle strong;

  @override
  Widget build(BuildContext context) => Row(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Padding(
        padding: const EdgeInsetsDirectional.only(top: 1, end: 8),
        child: Icon(
          _icon(
            context,
            Icons.warning_amber_rounded,
            CupertinoIcons.exclamationmark_triangle_fill,
          ),
          size: 20,
          color: Theme.of(context).colorScheme.error,
        ),
      ),
      Expanded(
        child: Text.rich(
          TextSpan(
            children: [
              TextSpan(text: 'Not enough space: ', style: strong),
              const TextSpan(text: 'this area needs '),
              TextSpan(text: size, style: strong),
              const TextSpan(text: ', and '),
              TextSpan(text: formatBytes(free), style: strong),
              const TextSpan(text: ' is free.'),
            ],
          ),
          style: style,
        ),
      ),
    ],
  );
}

/// The area's name: the date and time it was drawn, to keep or change.
class _NameField extends StatelessWidget {
  const _NameField({required this.controller});

  final TextEditingController controller;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (!isCupertinoOf(context)) {
      return TextField(
        controller: controller,
        textCapitalization: TextCapitalization.sentences,
        textInputAction: TextInputAction.done,
        decoration: const InputDecoration(labelText: 'Name', filled: true),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsetsDirectional.only(start: 4, bottom: 6),
          child: Text(
            'Name',
            style: theme.textTheme.bodySmall?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
        ),
        CupertinoTextField(
          controller: controller,
          textCapitalization: TextCapitalization.sentences,
          textInputAction: TextInputAction.done,
          clearButtonMode: OverlayVisibilityMode.editing,
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 11),
          style: theme.textTheme.bodyLarge,
          decoration: BoxDecoration(
            color: groupedRowColor(context),
            borderRadius: BorderRadius.circular(10),
          ),
        ),
      ],
    );
  }
}

/// The page's buttons, side by side and as wide as each other: the minor one at the
/// [start], the main one at the [end].
class _Buttons extends StatelessWidget {
  const _Buttons({required this.start, this.end});

  final Widget start;

  /// Null leaves its half empty.
  final Widget? end;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      Expanded(child: start),
      const SizedBox(width: 12),
      Expanded(child: end ?? const SizedBox.shrink()),
    ],
  );
}

/// [cupertino] on iOS, [material] elsewhere.
IconData _icon(BuildContext context, IconData material, IconData cupertino) =>
    isCupertinoOf(context) ? cupertino : material;
