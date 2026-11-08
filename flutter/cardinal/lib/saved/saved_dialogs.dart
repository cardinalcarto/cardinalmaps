import 'package:material_ui/material_ui.dart';

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

/// Edits an item's [name] and [note], returning them, or null if cancelled. [hint] is
/// what a blank name falls back on, if it's allowed to be blank.
class EditItemDialog extends StatefulWidget {
  const EditItemDialog({
    super.key,
    required this.title,
    required this.name,
    required this.note,
    this.hint,
  });

  final String title;
  final String name;
  final String? note;
  final String? hint;

  @override
  State<EditItemDialog> createState() => _EditItemDialogState();
}

class _EditItemDialogState extends State<EditItemDialog> {
  late final _name = TextEditingController(text: widget.name);
  late final _note = TextEditingController(text: widget.note);

  @override
  void dispose() {
    _name.dispose();
    _note.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final blankAllowed = widget.hint != null;
    return AlertDialog(
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
                : () =>
                      Navigator.of(context)
                          .pop((name: _name.text, note: _note.text)),
            child: const Text('Save'),
          ),
        ),
      ],
    );
  }
}
