/// What the directions sheet's transit views share: the chips for getting to stops, how
/// live the plan is, and the small pieces both the overview and a journey's details use.
///
/// The look follows Murena Maps, with less padding.
library;

import 'package:material_ui/material_ui.dart';

import 'format.dart';
import 'transit_plan.dart';

/// Chips for how to get to, from, and between stops.
class TransitAccessRow extends StatelessWidget {
  const TransitAccessRow({
    super.key,
    required this.access,
    required this.onChanged,
  });

  final TransitAccess access;
  final ValueChanged<TransitAccess> onChanged;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(12, 0, 12, 4),
    child: Wrap(
      spacing: 8,
      children: [
        for (final option in TransitAccess.values)
          ChoiceChip(
            avatar: Icon(accessIcon(option), size: 18),
            label: Text(option.label),
            selected: option == access,
            showCheckmark: false,
            visualDensity: VisualDensity.compact,
            onSelected: (_) => onChanged(option),
          ),
      ],
    ),
  );
}

IconData accessIcon(TransitAccess access) => switch (access) {
  TransitAccess.walking => Icons.directions_walk,
  TransitAccess.cycling => Icons.directions_bike,
  TransitAccess.wheelchair => Icons.accessible,
};

IconData streetIcon(String mode) => switch (mode) {
  'cycling' => Icons.directions_bike,
  'wheelchair' => Icons.accessible,
  _ => Icons.directions_walk,
};

/// What a street leg is called: the same words as the chips for getting to stops.
String streetVerb(String mode) => switch (mode) {
  'cycling' => TransitAccess.cycling.label,
  'wheelchair' => TransitAccess.wheelchair.label,
  _ => TransitAccess.walking.label,
};

IconData vehicleIcon(VehicleKind kind) => switch (kind) {
  VehicleKind.bus => Icons.directions_bus,
  VehicleKind.tram => Icons.tram,
  VehicleKind.subway => Icons.subway,
  VehicleKind.rail => Icons.train,
  VehicleKind.ferry => Icons.directions_boat,
  VehicleKind.cableway => Icons.airline_seat_recline_normal,
  VehicleKind.other => Icons.directions_transit,
};

/// Whether the plan's times are live, and from how many feeds.
class RealtimeLine extends StatelessWidget {
  const RealtimeLine({super.key, required this.plan});

  final TransitPlan plan;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final live = plan.feedsApplied > 0;
    final text = switch (plan) {
      TransitPlan(feedsReported: 0) => 'Scheduled times: no live data',
      TransitPlan(feedsApplied: 0) =>
        'Scheduled times: live data is out of date',
      TransitPlan(feedsReported: 1) => 'Live times from 1 real-time feed',
      _ when plan.feedsApplied == plan.feedsReported =>
        'Live times from ${plan.feedsReported} real-time feeds',
      _ =>
        'Live times from ${plan.feedsApplied} of ${plan.feedsReported} '
            'real-time feeds',
    };
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 2, 16, 4),
      child: Row(
        children: [
          Icon(
            live ? Icons.sensors : Icons.schedule,
            size: 16,
            color: live ? liveColor(theme) : theme.colorScheme.onSurfaceVariant,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              text,
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

/// What to say when a plan has no journeys.
String noJourneysMessage(TransitPlan plan, TransitAccess access) {
  final getting = switch (access) {
    TransitAccess.walking => 'walking',
    TransitAccess.cycling => 'cycling',
    TransitAccess.wheelchair => 'wheelchair',
  };
  return switch (plan.noJourneysReason) {
    NoJourneysReason.noAccess => 'No stops within $getting range of the start',
    NoJourneysReason.noEgress =>
      'No stops within $getting range of the destination',
    NoJourneysReason.slowerThanDirect =>
      plan.directSeconds != null
          ? 'Going there directly is faster '
                '(${formatDuration(plan.directSeconds!.toDouble())})'
          : 'Going there directly is faster',
    _ => 'No transit leaves in the next hour for this trip',
  };
}

Color liveColor(ThemeData theme) => theme.brightness == Brightness.dark
    ? const Color(0xff7fd99a)
    : const Color(0xff1b7a3a);

/// A clock time as the rider's settings show it, at a stop's UTC offset if it's known.
String clock(BuildContext context, DateTime time, {int? utcOffset}) {
  final local = utcOffset == null
      ? time.toLocal()
      : time.toUtc().add(Duration(seconds: utcOffset));
  return MaterialLocalizations.of(context).formatTimeOfDay(
    TimeOfDay(hour: local.hour, minute: local.minute),
    alwaysUse24HourFormat: MediaQuery.alwaysUse24HourFormatOf(context),
  );
}

/// The offset of the first stop the journey visits, for its times.
int? journeyUtcOffset(Journey journey) =>
    journey.rides.firstOrNull?.boarding.utcOffset;

/// How a ride's departure stands against its schedule.
({String text, Color color, bool live}) rideStatus(
  ThemeData theme,
  TransitLeg ride,
) {
  final muted = theme.colorScheme.onSurfaceVariant;
  if (!ride.realtime) return (text: 'Scheduled', color: muted, live: false);
  if (ride.tripState == 'added' || ride.tripState == 'replaced') {
    return (text: 'Live: changed trip', color: liveColor(theme), live: true);
  }
  final delay = ride.boarding.departureDelay ?? ride.delaySeconds ?? 0;
  final minutes = (delay.abs() / 60).round();
  if (minutes == 0) {
    return (text: 'Live: on time', color: liveColor(theme), live: true);
  }
  if (delay > 0) {
    return (
      text: 'Live: $minutes min late',
      color: theme.colorScheme.error,
      live: true,
    );
  }
  return (
    text: 'Live: $minutes min early',
    color: liveColor(theme),
    live: true,
  );
}

Color? parseRouteColor(String? rrggbb) {
  if (rrggbb == null || !RegExp(r'^[0-9a-fA-F]{6}$').hasMatch(rrggbb)) {
    return null;
  }
  return Color(0xff000000 | int.parse(rrggbb, radix: 16));
}

/// A moved stop, beside the walk to or from it: the agency only says where it went in
/// words.
class StopMovedBanner extends StatelessWidget {
  const StopMovedBanner({super.key, required this.notice, this.full = true});

  final StopMovedNotice notice;

  /// Whether to show the alert's whole description, not just its header.
  final bool full;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final name = notice.stopName ?? 'This stop';
    final header = notice.header;
    final description = notice.description;
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
      decoration: BoxDecoration(
        color: scheme.errorContainer,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.wrong_location, size: 20, color: scheme.onErrorContainer),
          const SizedBox(width: 8),
          Expanded(
            child: DefaultTextStyle.merge(
              style: TextStyle(color: scheme.onErrorContainer),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '$name has moved',
                    style: theme.textTheme.labelLarge?.copyWith(
                      color: scheme.onErrorContainer,
                    ),
                  ),
                  if (header != null)
                    Text(header, style: theme.textTheme.bodySmall),
                  if (full && description != null && description != header)
                    Padding(
                      padding: const EdgeInsets.only(top: 2),
                      child: Text(
                        description,
                        style: theme.textTheme.bodySmall,
                      ),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class IconLine extends StatelessWidget {
  const IconLine({
    super.key,
    required this.icon,
    required this.text,
    required this.style,
    this.color,
    this.maxLines = 1,
  });

  final IconData icon;
  final String text;
  final TextStyle? style;
  final Color? color;
  final int maxLines;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: 1),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(top: 1, right: 4),
          child: Icon(
            icon,
            size: 14,
            color: color ?? Theme.of(context).colorScheme.onSurfaceVariant,
          ),
        ),
        Expanded(
          child: Text(
            text,
            style: style,
            maxLines: maxLines,
            overflow: TextOverflow.ellipsis,
          ),
        ),
      ],
    ),
  );
}

class AccessibilityLine extends StatelessWidget {
  const AccessibilityLine({super.key, required this.accessibility});

  final Accessibility accessibility;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final (icon, text, color) = switch (accessibility) {
      Accessibility.accessible => (
        Icons.accessible,
        'Wheelchair accessible',
        liveColor(theme),
      ),
      Accessibility.inaccessible => (
        Icons.not_accessible,
        'Not wheelchair accessible',
        theme.colorScheme.error,
      ),
      Accessibility.unknown => (
        Icons.accessible,
        'Wheelchair access unknown',
        theme.colorScheme.onSurfaceVariant,
      ),
    };
    return IconLine(
      icon: icon,
      text: text,
      color: color,
      style: theme.textTheme.bodySmall?.copyWith(color: color),
    );
  }
}

/// GTFS-RT's occupancy status, as a rider would say it.
String occupancyText(String status) => switch (status) {
  'empty' => 'Empty',
  'many_seats_available' => 'Many seats',
  'few_seats_available' => 'Few seats',
  'standing_room_only' => 'Standing room only',
  'crushed_standing_room_only' => 'Very full',
  'full' => 'Full',
  'not_accepting_passengers' => 'Not taking riders',
  _ => status.replaceAll('_', ' '),
};
