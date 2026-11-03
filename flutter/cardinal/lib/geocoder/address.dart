import 'package:address_formatter/address_formatter.dart';

import '../store/store.dart';

/// The OpenCage address component each Who's On First placetype fills.
const _components = {
  'neighbourhood': 'neighbourhood',
  'microhood': 'neighbourhood',
  'macrohood': 'suburb',
  'borough': 'city_district',
  'locality': 'city',
  'localadmin': 'municipality',
  'county': 'county',
  'macrocounty': 'state_district',
  'region': 'state',
  'country': 'country',
};

/// [place]'s address formatted by its country's conventions (OpenCage's templates),
/// one line per element, without the country. Empty if it has no street address.
List<String> addressLines(Place place) {
  final address = place.address;
  if (address == null || (address.street == null && address.housenumber == null)) {
    return const [];
  }
  final formatted = formatAddress(
    {
      ..._adminComponents(place),
      'house_number': address.housenumber,
      'road': address.street,
      'postcode': address.postcode,
    },
    fallbackCountryCode: place.countryCode,
    appendCountry: false,
  );
  return formatted.split('\n').where((l) => l.isNotEmpty).toList();
}

/// The first line of a place's search result or card: its name, or for an address its
/// street address.
String placeTitle(Place place) {
  final name = place.name;
  if (name != null && name.isNotEmpty) return name;
  final lines = addressLines(place);
  return lines.isNotEmpty ? lines.first : place.id;
}

/// The second line: where the place is.
///
/// Its street address by its country's conventions (the rest of it, for an address,
/// whose first line is the title); else for an administrative area the areas containing
/// it, and for anything else its city and region, also by its country's conventions.
String? placeSubtitle(Place place) {
  final lines = addressLines(place);
  final named = place.name != null && place.name!.isNotEmpty;
  if (lines.isNotEmpty) {
    final rest = named ? lines : lines.skip(1);
    return rest.isEmpty ? null : rest.join(', ');
  }
  final admins = place.admins ?? const [];
  if (admins.isEmpty) return null;
  if (place.kind == 'admin') {
    return admins.map((a) => a.name).join(', ');
  }
  final formatted = formatAddressSingleLine(
    _adminComponents(place),
    fallbackCountryCode: place.countryCode,
    appendCountry: false,
  );
  return formatted.isEmpty ? null : formatted;
}

Map<String, String> _adminComponents(Place place) => {
  'country_code': ?place.countryCode,
  for (final admin in (place.admins ?? const <AdminRef>[]).reversed)
    ?_components[admin.placetype]: admin.name,
};
