All files in this directory are copied verbatim from the Valhalla repo,
because Git has no way of defining a partial submodule.

They come from `proto/descriptors/` at the `3.9.1` tag,
which is the Valhalla version Valinor is pinned to (see `docs/routing.md`).
When updating, copy every file from that directory at the new tag
and update the tag named here.

One field is ours and has to be added again after copying:
`Options.valinor_leg_details` (field 1000, in `options.proto`),
which carries the request option of the same name from the front to Tyr
(see `docs/osrm-leg-details.md`).
Valinor's own fields are numbered from 1000, clear of mainline's,
so a message with one still decodes in mainline, which keeps fields it doesn't know.

The Valhalla source is available at https://github.com/valhalla/valhalla,
and is available under the MIT license.
