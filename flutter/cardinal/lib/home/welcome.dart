import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoDialogAction;
import 'package:material_ui/material_ui.dart';

import '../store/store.dart';
import '../ui/adaptive.dart';
import '../ui/startup_offers.dart';

/// Whether the user has agreed to the first-launch maps disclaimer.
final welcomeAccepted = Setting.flag('welcome.accepted', false);

/// Shows the required welcome before other startup offers, once per installation.
class WelcomePrompt extends StatefulWidget {
  const WelcomePrompt({super.key, required this.child});

  final Widget child;

  @override
  State<WelcomePrompt> createState() => _WelcomePromptState();
}

class _WelcomePromptState extends State<WelcomePrompt> {
  bool _checked = false;
  bool _accepted = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_checked) return;
    _checked = true;
    final settings = StoreScope.of(context).settings;
    final answer = Completer<void>();
    StartupOffersScope.maybeOf(context)?.hold(answer.future);
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      try {
        await _check(settings);
      } finally {
        answer.complete();
      }
    });
  }

  Future<void> _check(SettingsStore settings) async {
    if (!await settings.get(welcomeAccepted)) {
      while (mounted) {
        final agreed = await showAdaptiveDialog<bool>(
          context: context,
          barrierDismissible: false,
          builder: (context) => _WelcomeDialog(settings: settings),
        );
        if (agreed != true) continue;
        break;
      }
    }
    if (mounted) setState(() => _accepted = true);
  }

  @override
  Widget build(BuildContext context) => AbsorbPointer(
    absorbing: !_accepted,
    child: ExcludeFocus(excluding: !_accepted, child: widget.child),
  );
}

/// Both stages share one modal route, so nothing can open between them.
class _WelcomeDialog extends StatefulWidget {
  const _WelcomeDialog({required this.settings});

  final SettingsStore settings;

  @override
  State<_WelcomeDialog> createState() => _WelcomeDialogState();
}

class _WelcomeDialogState extends State<_WelcomeDialog> {
  bool _disclaimer = false;
  bool _saving = false;
  bool _saveFailed = false;

  Future<void> _agree() async {
    setState(() {
      _saving = true;
      _saveFailed = false;
    });
    try {
      await widget.settings.set(welcomeAccepted, true);
    } on Object catch (error) {
      debugPrint('welcome: saving acceptance: $error');
      if (mounted)
        setState(() {
          _saving = false;
          _saveFailed = true;
        });
      return;
    }
    if (mounted) Navigator.pop(context, true);
  }

  Widget _action(
    String label,
    VoidCallback? onPressed, {
    bool isDefault = false,
  }) => isCupertinoOf(context)
      ? CupertinoDialogAction(
          isDefaultAction: isDefault,
          onPressed: onPressed,
          child: Text(label),
        )
      : TextButton(onPressed: onPressed, child: Text(label));

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: false,
    child: AlertDialog.adaptive(
      scrollable: true,
      title: Text(_disclaimer ? 'Use maps safely' : 'Welcome to Cardinal Maps'),
      content: Text(
        _disclaimer
            ? 'Maps and directions may be inaccurate, incomplete, or out of date. '
                  'Always use your own judgment, follow local laws and posted signs, '
                  'and pay attention to your surroundings. Do not use the app in a '
                  'way that distracts you while driving.'
                  '${_saveFailed ? "\n\nCouldn't save your agreement. Please try again." : ""}'
            : 'To use Cardinal Maps, you currently need to download offline maps. '
                  'Tap Offline in the bottom-right corner of the screen, then choose '
                  'Download an area to get started.',
      ),
      actions: [
        if (_disclaimer) ...[
          _action(
            'Back',
            _saving ? null : () => setState(() => _disclaimer = false),
          ),
          _action('I agree', _saving ? null : _agree, isDefault: true),
        ] else
          _action(
            'Next',
            () => setState(() => _disclaimer = true),
            isDefault: true,
          ),
      ],
    ),
  );
}
