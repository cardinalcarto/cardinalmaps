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
          if (fits)
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
            Row(
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
                    color: colors.error,
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
                        TextSpan(text: formatBytes(free ?? 0), style: strong),
                        const TextSpan(text: ' is free.'),
                      ],
                    ),
                    style: body,
                  ),
                ),
              ],
            ),
          const SizedBox(height: 16),
          _NameField(controller: name),
          if (hasTransit) ...[
            const SizedBox(height: 12),
            Text(
              "You'll choose transit next.",
              style: theme.textTheme.bodyMedium?.copyWith(
                color: colors.onSurfaceVariant,
              ),
            ),
          ],
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

/// Page 2 of the confirmation, still to be built: transit. For now its title and
/// Back, so the pages and the way between them can be tried.
class TransitStep extends StatelessWidget {
  const TransitStep({super.key, required this.onBack});

  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: _padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text('Transit', style: theme.textTheme.headlineSmall),
          const SizedBox(height: 8),
          Text(
            'Choosing transit comes in the next piece.',
            style: theme.textTheme.bodyLarge?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: 20),
          _Buttons(
            start: AppButton(
              kind: AppButtonKind.tinted,
              label: 'Back',
              onPressed: onBack,
            ),
          ),
        ],
      ),
    );
  }
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
