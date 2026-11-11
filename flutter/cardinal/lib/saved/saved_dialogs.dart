import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoAlertDialog,
        CupertinoButton,
        CupertinoDialogAction,
        CupertinoTextField;
import 'package:material_ui/material_ui.dart';

import '../ui/adaptive.dart';

import 'emoji.dart';

/// Asks for a new list's name, returning it, or null if cancelled.
class NewListDialog extends StatefulWidget {
  const NewListDialog({super.key});

  @override
  State<NewListDialog> createState() => _NewListDialogState();
}

class _NewListDialogState extends State<NewListDialog> {
  final _name = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _create() {
    final name = _name.text.trim();
    if (name.isNotEmpty) Navigator.of(context).pop(name);
  }

  @override
  Widget build(BuildContext context) {
    if (isCupertinoOf(context)) {
      return CupertinoAlertDialog(
        title: const Text('New List'),
        content: Padding(
          padding: const EdgeInsets.only(top: 12),
          child: CupertinoTextField(
            controller: _name,
            autofocus: true,
            placeholder: 'Name',
            textCapitalization: TextCapitalization.sentences,
            onSubmitted: (_) => _create(),
          ),
        ),
        actions: [
          CupertinoDialogAction(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          ListenableBuilder(
            listenable: _name,
            builder: (context, _) => CupertinoDialogAction(
              isDefaultAction: true,
              onPressed: _name.text.trim().isEmpty ? null : _create,
              child: const Text('Create'),
            ),
          ),
        ],
      );
    }
    return _materialDialog(context);
  }

  Widget _materialDialog(BuildContext context) => AlertDialog(
    title: const Text('New list'),
    content: TextField(
      controller: _name,
      autofocus: true,
      textCapitalization: TextCapitalization.sentences,
      decoration: const InputDecoration(labelText: 'Name'),
      onSubmitted: (_) => _create(),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      ListenableBuilder(
        listenable: _name,
        builder: (context, _) => TextButton(
          onPressed: _name.text.trim().isEmpty ? null : _create,
          child: const Text('Create'),
        ),
      ),
    ],
  );
}

/// Emoji offered without opening the keyboard: what places on a map tend to be.
const commonEmoji = [
  '🏠', '💼', '⭐', '❤️', '☕', '🍕', '🍺', '🌲', //
  '⛰️', '🏕️', '🏖️', '🏛️', '🛒', '⛽', '🍦', '🍣', //
];

/// Edits an item's [name] and [note], and for a place its [emoji], returning them, or
/// null if cancelled. [hint] is what a blank name falls back on, if it's allowed to be
/// blank.
class EditItemDialog extends StatefulWidget {
  const EditItemDialog({
    super.key,
    required this.title,
    required this.name,
    required this.note,
    this.hint,
    this.editsEmoji = false,
    this.emoji,
  });

  final String title;
  final String name;
  final String? note;
  final String? hint;

  /// Whether it offers an emoji: places have them, lists don't.
  final bool editsEmoji;
  final String? emoji;

  @override
  State<EditItemDialog> createState() => _EditItemDialogState();
}

class _EditItemDialogState extends State<EditItemDialog> {
  late final _name = TextEditingController(text: widget.name);
  late final _note = TextEditingController(text: widget.note);
  late String? _emoji = widget.emoji;

  /// Shows the emoji if the grid doesn't.
  late final _typed = TextEditingController(
    text: commonEmoji.contains(_emoji) ? '' : _emoji,
  );

  @override
  void dispose() {
    _name.dispose();
    _note.dispose();
    _typed.dispose();
    super.dispose();
  }

  void _pick(String? emoji) {
    _typed.clear();
    setState(() => _emoji = emoji);
  }

  /// Only an emoji counts: the last one typed. Emptying the field takes its emoji away.
  void _typing(String text) {
    final emoji = lastEmoji(text);
    final kept = emoji ?? '';
    if (text != kept) {
      _typed.value = TextEditingValue(
        text: kept,
        selection: TextSelection.collapsed(offset: kept.length),
      );
    }
    if (emoji != null || !commonEmoji.contains(_emoji)) {
      setState(() => _emoji = emoji);
    }
  }

  /// The emoji: a grid of the common ones, and a field for any other. On iOS it goes
  /// in a section of its own, which [cupertino] says, headed there by the section.
  Widget _emojiField(BuildContext context, {bool cupertino = false}) {
    final scheme = Theme.of(context).colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (!cupertino) ...[
          const SizedBox(height: 16),
          Text('Emoji', style: Theme.of(context).textTheme.labelLarge),
          const SizedBox(height: 4),
        ],
        Wrap(
          children: [
            for (final emoji in commonEmoji)
              InkWell(
                borderRadius: BorderRadius.circular(8),
                onTap: () => _pick(emoji),
                child: Container(
                  width: 36,
                  height: 36,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(8),
                    color: _emoji == emoji
                        ? (cupertino
                              ? scheme.primary.withValues(alpha: 0.15)
                              : scheme.secondaryContainer)
                        : null,
                  ),
                  child: Text(emoji, style: const TextStyle(fontSize: 20)),
                ),
              ),
          ],
        ),
        Row(
          children: [
            Expanded(
              child: cupertino
                  ? CupertinoTextField(
                      controller: _typed,
                      onChanged: _typing,
                      placeholder: 'Any emoji',
                    )
                  : TextField(
                      controller: _typed,
                      onChanged: _typing,
                      decoration: const InputDecoration(
                        labelText: 'Any emoji',
                        isDense: true,
                      ),
                    ),
            ),
            if (cupertino)
              CupertinoButton(
                onPressed: _emoji == null ? null : () => _pick(null),
                child: const Text('None'),
              )
            else
              TextButton(
                onPressed: _emoji == null ? null : () => _pick(null),
                child: const Text('None'),
              ),
          ],
        ),
      ],
    );
  }

  /// Whether what's entered can be saved: a name, unless a blank one is allowed.
  bool get _canSave => widget.hint != null || _name.text.trim().isNotEmpty;

  void _save() =>
      Navigator.of(context)
          .pop((name: _name.text, note: _note.text, emoji: _emoji));

  /// iOS's form sheet: the fields grouped, Cancel and Save in the bar at the top.
  Widget _cupertinoForm(BuildContext context) {
    Widget field(Widget child) => Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: child,
    );
    return Padding(
      padding: EdgeInsets.only(bottom: MediaQuery.viewInsetsOf(context).bottom),
      child: SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListenableBuilder(
              listenable: _name,
              builder: (context, _) => AppSheetBar(
                title: widget.title,
                onCancel: () => Navigator.of(context).pop(),
                done: 'Save',
                onDone: _canSave ? _save : null,
              ),
            ),
            Flexible(
              child: SingleChildScrollView(
                child: Column(
                  children: [
                    AppListSection(
                      children: [
                        field(
                          CupertinoTextField.borderless(
                            controller: _name,
                            autofocus: true,
                            padding: EdgeInsets.zero,
                            placeholder: widget.hint ?? 'Name',
                          ),
                        ),
                        field(
                          CupertinoTextField.borderless(
                            controller: _note,
                            padding: EdgeInsets.zero,
                            placeholder: 'Note',
                            maxLines: null,
                          ),
                        ),
                      ],
                    ),
                    if (widget.editsEmoji)
                      AppListSection(
                        header: 'Emoji',
                        children: [
                          field(_emojiField(context, cupertino: true)),
                        ],
                      ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (isCupertinoOf(context)) return _cupertinoForm(context);
    final blankAllowed = widget.hint != null;
    return AlertDialog(
      scrollable: true,
      title: Text(widget.title),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            controller: _name,
            autofocus: true,
            decoration: InputDecoration(
              labelText: 'Name',
              hintText: widget.hint,
            ),
          ),
          TextField(
            controller: _note,
            decoration: const InputDecoration(labelText: 'Note'),
            maxLines: null,
          ),
          if (widget.editsEmoji) _emojiField(context),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        ListenableBuilder(
          listenable: _name,
          builder: (context, _) => TextButton(
            onPressed: !blankAllowed && _name.text.trim().isEmpty
                ? null
                : _save,
            child: const Text('Save'),
          ),
        ),
      ],
    );
  }
}

/// Shows [form], an [EditItemDialog] say: on iOS a sheet, as iOS asks for more than an
/// alert holds; elsewhere a dialog.
Future<T?> showFormDialog<T>(BuildContext context, Widget form) =>
    isCupertinoOf(context)
    ? showAppSheet<T>(context: context, builder: (_) => form)
    : showDialog<T>(context: context, builder: (_) => form);
