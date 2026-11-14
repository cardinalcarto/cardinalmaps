import '../store/store.dart';

/// Whether directions go by wheelchair unless you choose otherwise: on foot is by
/// wheelchair, and so is getting to and from transit stops until another way is chosen.
final wheelchairByDefault = Setting.flag('accessibility.wheelchair', false);
