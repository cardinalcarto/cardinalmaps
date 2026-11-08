# Offline regions (proposal)

Status: proposal, not built.
It follows the decision in [flutter-api.md](flutter-api.md):
every offline region is cut from the same planet build,
a device opens all of its regions as one router,
and the router refuses to mix builds (the build ID in each tile's checksum).

The data pipeline's `RoutingTiles` builds one tile set per release,
as `routing/tiles.tar` (see `data_pipeline/README.md`).
This proposal covers turning that tile set into region downloads.
Nothing in the app defines regions yet, so where the region list comes from is left open.

## Selecting tiles

A region is a named (multi)polygon.
Its tiles are every tile of the release, at each of the three levels,
whose bounding box intersects the polygon.
The levels' tiles are 4° (level 0, motorways and trunks), 1° (level 1, arterials),
and 0.25° (level 2, local roads) on a side.

Mainline's own extract tool already selects tiles this way and cuts from an existing tarball:

```shell
valhalla_build_extract -c valhalla.json -e region.tar -g polygons/   # or -b minx,miny,maxx,maxy
```

With `-e`, it reads the tiles from `mjolnir.tile_extract` (the release's `tiles.tar`)
and writes only the tiles that intersect the polygons in `polygons/*.geojson`, or the bounding box given with `-b`.
I tried this on the Andorra build: a bounding box over the west of Andorra kept 6 of its 7 tiles,
the tiles' bytes were identical to the release's,
and mainline routed over the cut extract with the same result as over the full one.

How the selection behaves:

- **Every level.** A region's level 0 tiles reach up to 4° beyond it,
  so motorways and trunks connect well past its border,
  while local roads stop within a level 2 tile (0.25°) of it.
- **Buffer the polygon first.** Tile granularity gives some slack at the border, but there's no guarantee.
  The proposal is to buffer each polygon by a fixed distance before cutting (10 km to start),
  so that destinations just across a border still snap to a road.
- **Only exterior rings.** Mainline ignores holes, which only adds tiles, so that's safe.
  Polygons that cross the antimeridian must be split first, since tile bounding boxes don't wrap.
- **Missing tiles are expected.** A route that leaves the region reaches edges
  whose end nodes are in tiles the device doesn't have.
  The tarball provider reports these as `TileNotAvailable`.
  Loki and Thor must treat that the way mainline's `GraphReader` treats a missing tile:
  as no edges, not as an error.
  This needs a test, with a cut extract, before regions ship.

## Tarball format

A region tarball is exactly `valhalla_build_extract`'s format, the same as the release's `tiles.tar`:

- an ordinary tar file whose first entry is `index.bin`,
  holding 16-byte `<QLL` entries (data offset, tile ID, and size);
- then one entry per tile, named by its tile path (`2/000/763/927.gph`).

Tiles are copied byte for byte, so the build ID and the checksum in every header stay as built.
That means three things:

- mainline and Valinor's `TarballTileProvider` both open a region without changes;
- `check_build_ids` on a region gives the release's build ID;
- `tar x` gives an ordinary mainline tile directory.

A region tarball isn't byte-reproducible.
Mainline stamps `index.bin` with the current time, and it doesn't order tiles canonically.
So identify a download by the release, the region, and the SHA-256 recorded when it was cut,
not by hashing a re-cut.
If reproducible cuts turn out to matter (for example, for content-addressed hosting),
a small cutter could write the same format from `index.bin`, in one pass for every region,
instead of one scan of the planet tarball per region.
Start with mainline's tool.

## What a region download is

For each release, the pipeline would write `routing/regions/` next to `tiles.tar`:

```text
routing/regions/index.json        release, build ID, Valhalla version, and the regions
routing/regions/<region-id>.tar   one tarball per region
```

`index.json` would list, for each region:

- its ID and name;
- the polygon it was cut from (or that polygon's bounding box);
- its tile count, byte size, and SHA-256.

The files would be served statically, for example by the proxy under `/routing/regions/`, or from object storage.

On the device, a region download:

1. fetches the tarball to a temporary file;
2. checks its SHA-256 against `index.json`;
3. checks with `check_build_ids` that every tile has the index's build ID;
4. renames the file into place, atomically, as `TarballTileProvider` requires.

Since regions can't mix builds, updating to a new release means downloading every installed region from it.
The device keeps routing on the old set until all of the new set has been verified, and then swaps them together.
Neighboring regions share border tiles and most level 0 and level 1 tiles.
The router opens them as one tile set, so a tile can come from any region that has it.

In the pipeline this would be a `RoutingRegions` stage after `RoutingTiles`.
It would take the routing directory and a directory of region GeoJSON,
and run `valhalla_build_extract -e … -g …` per region in the pinned image.
`release.sh` would record the regions in the manifest.

## Open questions

1. **Where regions come from.** One option is a GeoJSON catalog committed to the repository.
   Another is Geofabrik's extract boundaries, which `valhalla_build_extract -r` already fetches,
   but that means a network fetch at build time and a dependency on an outside index.
2. **Duplicated level 0 and level 1 tiles.** The duplication between regions hasn't been measured.
   If it's large, those levels could be a shared "base" download, with regions carrying only level 2.
   That changes what a region is, so measure first, on a planet build.
3. **Buffer distance and region size limits.** Both depend on the app's storage budget.
4. **Hosting:** the proxy, or object storage, with or without range requests.
