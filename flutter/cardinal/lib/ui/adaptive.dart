/// Controls that are Cupertino on iOS and macOS and Material elsewhere, chosen by the
/// theme's platform (see `appTheme`), so a test can pick either.
library;

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoButton,
        CupertinoButtonSize,
        CupertinoDialogAction,
        CupertinoIcons,
        CupertinoListSection,
        CupertinoListTile,
        CupertinoListTileChevron,
        CupertinoSlidingSegmentedControl;
import 'package:material_ui/material_ui.dart';

import 'haptics.dart';
import 'theme.dart';

/// Whether the theme's platform gets the iOS look.
bool isCupertinoOf(BuildContext context) =>
    isCupertino(Theme.of(context).platform);

/// How prominent an [AppButton] is.
enum AppButtonKind {
  /// The accent filled in: the one main action.
  filled,

  /// A wash of the accent: actions beside the main one.
  tinted,

  /// Just the label: minor actions.
  plain,
}

/// A button with a [label] and maybe an [icon] before it.
class AppButton extends StatelessWidget {
  const AppButton({
    super.key,
    required this.label,
    required this.onPressed,
    this.icon,
    this.kind = AppButtonKind.filled,
  });

  final String label;
  final IconData? icon;
  final AppButtonKind kind;

  /// Null disables the button.
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    if (isCupertinoOf(context)) {
      final child = icon == null
          ? Text(label)
          : Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(icon, size: 20),
                const SizedBox(width: 6),
                Flexible(child: Text(label, overflow: TextOverflow.ellipsis)),
              ],
            );
      return switch (kind) {
        AppButtonKind.filled => CupertinoButton.filled(
          sizeStyle: CupertinoButtonSize.medium,
          onPressed: onPressed,
          child: child,
        ),
        AppButtonKind.tinted => CupertinoButton.tinted(
          sizeStyle: CupertinoButtonSize.medium,
          onPressed: onPressed,
          child: child,
        ),
        AppButtonKind.plain => CupertinoButton(
          sizeStyle: CupertinoButtonSize.medium,
          onPressed: onPressed,
          child: child,
        ),
      };
    }
    final text = Text(label);
    final glyph = icon == null ? null : Icon(icon);
    return switch (kind) {
      AppButtonKind.filled =>
        glyph == null
            ? FilledButton(onPressed: onPressed, child: text)
            : FilledButton.icon(onPressed: onPressed, icon: glyph, label: text),
      AppButtonKind.tinted =>
        glyph == null
            ? FilledButton.tonal(onPressed: onPressed, child: text)
            : FilledButton.tonalIcon(
                onPressed: onPressed,
                icon: glyph,
                label: text,
              ),
      AppButtonKind.plain =>
        glyph == null
            ? TextButton(onPressed: onPressed, child: text)
            : TextButton.icon(onPressed: onPressed, icon: glyph, label: text),
    };
  }
}

/// Rows grouped together: an inset grouped list on iOS, a card of rows elsewhere.
class AppListSection extends StatelessWidget {
  const AppListSection({
    super.key,
    required this.children,
    this.header,
    this.footer,
  });

  /// Usually [AppListTile]s.
  final List<Widget> children;
  final String? header;
  final String? footer;

  @override
  Widget build(BuildContext context) {
    if (isCupertinoOf(context)) {
      return CupertinoListSection.insetGrouped(
        // The sheet is the background.
        backgroundColor: const Color(0x00000000),
        decoration: BoxDecoration(
          color: groupedRowColor(context),
          borderRadius: BorderRadius.circular(10),
        ),
        // The margin above is for a header; without one, just a little room.
        topMargin: header == null ? 8 : null,
        // iOS heads and foots grouped rows in small grey text.
        header: header == null
            ? null
            : Text(header!.toUpperCase(), style: _sectionNote(context)),
        footer: footer == null
            ? null
            : Text(footer!, style: _sectionNote(context)),
        children: children,
      );
    }
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (header case final header?)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
              child: Text(header, style: theme.textTheme.titleSmall),
            ),
          Card.filled(
            margin: EdgeInsets.zero,
            clipBehavior: Clip.antiAlias,
            child: Column(
              children: [
                for (final (i, child) in children.indexed) ...[
                  if (i > 0) const Divider(height: 1, indent: 16),
                  child,
                ],
              ],
            ),
          ),
          if (footer case final footer?)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
              child: Text(
                footer,
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// The color of grouped rows on a sheet: white over the light sheet, and a step
/// lighter than the dark one, as iOS raises rows on an elevated sheet.
Color groupedRowColor(BuildContext context) {
  final colors = Theme.of(context).colorScheme;
  return Theme.of(context).brightness == Brightness.dark
      ? colors.surfaceContainerHigh
      : colors.surface;
}

/// The style of an [AppListSection]'s header and footer on iOS: Footnote, secondary.
TextStyle? _sectionNote(BuildContext context) {
  final theme = Theme.of(context);
  return theme.textTheme.bodySmall?.copyWith(
    color: theme.colorScheme.onSurfaceVariant,
  );
}

/// A row of an [AppListSection].
class AppListTile extends StatelessWidget {
  const AppListTile({
    super.key,
    required this.title,
    this.subtitle,
    this.leading,
    this.trailing,
    this.onTap,
    this.chevron = false,
    this.padding,
  });

  final Widget title;
  final Widget? subtitle;
  final Widget? leading;

  /// Shown at the end, instead of the chevron.
  final Widget? trailing;
  final VoidCallback? onTap;

  /// Whether a chevron at the end says the row opens something.
  final bool chevron;

  /// Around the row's content, if not the platform's own.
  final EdgeInsetsGeometry? padding;

  @override
  Widget build(BuildContext context) {
    if (isCupertinoOf(context)) {
      return CupertinoListTile(
        title: title,
        subtitle: subtitle,
        leading: leading,
        trailing:
            trailing ?? (chevron ? const CupertinoListTileChevron() : null),
        onTap: onTap,
        padding: padding,
      );
    }
    return ListTile(
      title: title,
      subtitle: subtitle,
      leading: leading,
      trailing: trailing ?? (chevron ? const Icon(Icons.chevron_right) : null),
      onTap: onTap,
      contentPadding: padding,
    );
  }
}

/// A choice of one of [segments], keyed by their values.
class AppSegmentedControl<T extends Object> extends StatelessWidget {
  const AppSegmentedControl({
    super.key,
    required this.segments,
    required this.selected,
    required this.onChanged,
  }) : assert(segments.length >= 2);

  final Map<T, Widget> segments;
  final T selected;
  final ValueChanged<T> onChanged;

  void _change(T value) {
    if (value == selected) return;
    Haptics.selection();
    onChanged(value);
  }

  @override
  Widget build(BuildContext context) {
    if (isCupertinoOf(context)) {
      return CupertinoSlidingSegmentedControl<T>(
        groupValue: selected,
        children: segments,
        onValueChanged: (value) {
          if (value != null) _change(value);
        },
      );
    }
    return SegmentedButton<T>(
      showSelectedIcon: false,
      segments: [
        for (final MapEntry(key: value, value: label) in segments.entries)
          ButtonSegment(value: value, label: label),
      ],
      selected: {selected},
      onSelectionChanged: (values) => _change(values.single),
    );
  }
}

/// A button of an [showAppDialog], which closes the dialog with [value].
class AppDialogAction<T> {
  const AppDialogAction({
    required this.label,
    required this.value,
    this.isDefault = false,
    this.isDestructive = false,
  });

  final String label;
  final T value;

  /// The action Return takes, in bold on iOS.
  final bool isDefault;

  /// An action that deletes or can't be undone, in red.
  final bool isDestructive;
}

/// Shows an alert: Cupertino's on iOS, Material's elsewhere. Completes with the value
/// of the action tapped, or null if it was dismissed.
Future<T?> showAppDialog<T>({
  required BuildContext context,
  required String title,
  String? message,
  Widget? content,
  required List<AppDialogAction<T>> actions,
}) {
  assert(message == null || content == null);
  return showAdaptiveDialog<T>(
    context: context,
    builder: (context) {
      final cupertino = isCupertinoOf(context);
      final error = Theme.of(context).colorScheme.error;
      return AlertDialog.adaptive(
        title: Text(title),
        content: content ?? (message == null ? null : Text(message)),
        actions: [
          for (final action in actions)
            if (cupertino)
              CupertinoDialogAction(
                isDefaultAction: action.isDefault,
                isDestructiveAction: action.isDestructive,
                onPressed: () => Navigator.pop(context, action.value),
                child: Text(action.label),
              )
            else
              TextButton(
                onPressed: () => Navigator.pop(context, action.value),
                child: Text(
                  action.label,
                  style: action.isDestructive ? TextStyle(color: error) : null,
                ),
              ),
        ],
      );
    },
  );
}

/// Closes a sheet or page: on iOS a glyph in a small grey circle, as Apple closes its
/// cards; elsewhere an icon button.
class AppCloseButton extends StatelessWidget {
  const AppCloseButton({super.key, required this.onPressed});

  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    if (!isCupertinoOf(context)) {
      return IconButton(
        tooltip: 'Close',
        icon: const Icon(Icons.close),
        onPressed: onPressed,
      );
    }
    final colors = Theme.of(context).colorScheme;
    return Semantics(
      button: true,
      label: 'Close',
      excludeSemantics: true,
      child: CupertinoButton(
        padding: EdgeInsets.zero,
        minimumSize: const Size.square(44),
        onPressed: onPressed,
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: colors.onSurface.withValues(alpha: 0.08),
            shape: BoxShape.circle,
          ),
          child: SizedBox.square(
            dimension: 30,
            child: Icon(
              CupertinoIcons.xmark,
              size: 14,
              weight: 700,
              color: colors.onSurfaceVariant,
            ),
          ),
        ),
      ),
    );
  }
}

/// One of a row of actions, as a place card has: on iOS a rounded button with its
/// [icon] over its [label], made to share a row equally; elsewhere a compact button with
/// the icon before the label, filled if [kind] is filled and outlined otherwise.
class AppActionButton extends StatelessWidget {
  const AppActionButton({
    super.key,
    required this.icon,
    required this.label,
    required this.onPressed,
    this.kind = AppButtonKind.tinted,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onPressed;
  final AppButtonKind kind;

  @override
  Widget build(BuildContext context) {
    if (!isCupertinoOf(context)) {
      const style = ButtonStyle(visualDensity: VisualDensity.compact);
      return kind == AppButtonKind.filled
          ? FilledButton.icon(
              onPressed: onPressed,
              style: style,
              icon: Icon(icon),
              label: Text(label),
            )
          : OutlinedButton.icon(
              onPressed: onPressed,
              style: style,
              icon: Icon(icon),
              label: Text(label),
            );
    }
    final theme = Theme.of(context);
    final filled = kind == AppButtonKind.filled;
    final color = filled
        ? theme.colorScheme.onPrimary
        : theme.colorScheme.primary;
    final child = Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 22, color: color),
        const SizedBox(height: 2),
        Text(
          label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.labelMedium?.copyWith(
            color: color,
            fontWeight: FontWeight.w600,
          ),
        ),
      ],
    );
    const padding = EdgeInsets.symmetric(vertical: 8, horizontal: 4);
    final radius = BorderRadius.circular(14);
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: filled
          ? CupertinoButton.filled(
              padding: padding,
              borderRadius: radius,
              minimumSize: const Size(0, 56),
              onPressed: onPressed,
              child: child,
            )
          : CupertinoButton.tinted(
              padding: padding,
              borderRadius: radius,
              minimumSize: const Size(0, 56),
              onPressed: onPressed,
              child: child,
            ),
    );
  }
}

/// The top of a sheet that asks for something: on iOS its title centered between
/// [cancel] at the start and the [done] action at the end, as iOS heads its sheets;
/// elsewhere just the title, the actions being the sheet's own buttons below.
class AppSheetBar extends StatelessWidget {
  const AppSheetBar({
    super.key,
    required this.title,
    this.cancel = 'Cancel',
    this.onCancel,
    this.done = 'Done',
    this.onDone,
  });

  final String title;
  final String cancel;

  /// Null leaves Cancel out.
  final VoidCallback? onCancel;
  final String done;

  /// Null disables [done].
  final VoidCallback? onDone;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (!isCupertinoOf(context)) {
      return Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
        child: Text(title, style: theme.textTheme.titleMedium),
      );
    }
    return SizedBox(
      height: 44,
      child: NavigationToolbar(
        middleSpacing: 8,
        leading: onCancel == null
            ? null
            : CupertinoButton(
                padding: const EdgeInsets.symmetric(horizontal: 16),
                onPressed: onCancel,
                child: Text(cancel),
              ),
        middle: Text(
          title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: theme.textTheme.titleMedium,
        ),
        trailing: CupertinoButton(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          onPressed: onDone,
          child: Text(
            done,
            style: const TextStyle(fontWeight: FontWeight.w600),
          ),
        ),
      ),
    );
  }
}

/// Shows a sheet of [builder]'s, above the app's own sheet: iOS's sheet on iOS, through
/// the theme, and Material's elsewhere.
Future<T?> showAppSheet<T>({
  required BuildContext context,
  required WidgetBuilder builder,
}) => showModalBottomSheet<T>(
  context: context,
  isScrollControlled: true,
  // Above the app's sheet, whose pages must stay current.
  useRootNavigator: true,
  showDragHandle: true,
  builder: builder,
);
