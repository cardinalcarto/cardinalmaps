import 'package:material_ui/material_ui.dart';

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
  Widget build(BuildContext context) => AlertDialog(
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

  Widget _emojiField(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 16),
        Text('Emoji', style: Theme.of(context).textTheme.labelLarge),
        const SizedBox(height: 4),
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
                    color: _emoji == emoji ? scheme.secondaryContainer : null,
                  ),
                  child: Text(emoji, style: const TextStyle(fontSize: 20)),
                ),
              ),
          ],
        ),
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _typed,
                onChanged: _typing,
                decoration: const InputDecoration(
                  labelText: 'Any emoji',
                  isDense: true,
                ),
              ),
            ),
            TextButton(
              onPressed: _emoji == null ? null : () => _pick(null),
              child: const Text('None'),
            ),
          ],
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
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
                : () => Navigator.of(context)
                      .pop((name: _name.text, note: _note.text, emoji: _emoji)),
            child: const Text('Save'),
          ),
        ),
      ],
    );
  }
}
