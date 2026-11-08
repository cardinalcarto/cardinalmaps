The JSON files in this directory are copied verbatim from the Valhalla repo,
where they are generated from the gettext `.po` files next to them.

They come from `locales/` at the `3.9.1` tag,
which is the Valhalla version Valinor is pinned to (see `docs/routing.md`).
When updating, copy every `*.json` file from that directory at the new tag,
update the tag named here,
and update the locale table in `src/narrative/dictionary.rs`
(its tests check the table against these files).

The Valhalla source is available at https://github.com/valhalla/valhalla,
and is available under the MIT license.
