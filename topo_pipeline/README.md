# topo_pipeline

Turns DEMs into contour lines for Cardinal Topo, and gets them into the
`planetiler-openmaptiles` fork's input as OSM ways. The tile layers that read them are
`contour_m` and `contour_ft`, in `addons/Contour.java` there.

```
topo contours --config topo.toml --work work/ --out contours.osm.pbf [--bbox w,s,e,n]
topo append   --planet planet.osm.pbf --contours contours.osm.pbf --out planet-topo.osm.pbf
topo seams    --config topo.toml --work work/ [--bbox w,s,e,n]
```

then run planetiler with `--osm-path=planet-topo.osm.pbf`. The Dagger module wraps these as
`contours`, `append-contours` and `topo-tiles`; see `data_pipeline/README.md`.

## DEMs

`topo.example.toml` lists sources from lowest to highest priority. A source's rasters are
found by walking `dir` for files ending in `suffix`, so downloaded archives can stay as they
unpack. JAXA's AW3D30 `*_DSM.tif` files work directly; their `*_MSK.tif` and `*_STK.tif` files
aren't read.

A raster must be a single-band GeoTIFF in geographic coordinates, on a lattice of 1 arc
second divided by an odd integer (1, 1/3, 1/9), with its pixel centers on that lattice.
Others are refused, or, for those in a projection, need reprojecting first:

```
gdalwarp -t_srs EPSG:4326 -tr 0.0000925925925 0.0000925925925 -tap -r bilinear -of COG in.tif out.tif
```

A source can instead read zip archives of rasters from an S3-compatible bucket, so that none
of it has to fit on disk; see `topo.example.toml`. `scripts/mirror_aw3d30.py` puts JAXA's data
in one. Each raster is fetched by itself with a range request, and chunks are worked through
5 degree block by block, which keeps `cache_gb` of decoded rasters useful. Set `cache_gb` to at
least 8. The output is the same as from a directory of the same files, and chunks finished one
way are valid the other way too. JAXA requires that anything published from AW3D30 credit JAXA.

The datum isn't converted. NAD83 and WGS84 are treated alike (about a meter apart in the
contiguous US), as are the sources' vertical datums. `feather` hides such differences at a
source's edge, by blending over that many cells inside it. Where a high resolution source
meets JAXA's it also hides this: **AW3D30 is a surface model, so its elevations include the
forest canopy**, while most high resolution DEMs are bare earth. In tall forest, expect JAXA
contours to run high by up to the height of the trees.

`median` removes speckle and `sigma` smooths, in cells of the source's own lattice. Keep
`sigma` modest. At 2 on AW3D30 it merged separate knobs and flattened small saddles.

## How the contours match up

The planet is worked on in quarter degree chunks, which only ever depend on their
surroundings, never on each other or the order they run in:

- Every lattice is global, with its points at `(i + 0.5) / (3600 r)` degrees from (-180, -90).
  Sources are filtered in their own lattice, in windows with enough margin that a value comes
  out identical in any chunk. So two chunks computing the same point get the same bits.
- A crossing is always computed from the same end of a lattice edge, so the ends of a line on
  a shared boundary are identical, not just close. Simplifying and rounding corners keep ends
  fixed. Planetiler then merges lines at these ends, because they're at the same coordinates.
- Lines are traced in a buffered window and only then cut to the chunk, so choices about a
  line (a small closed loop gets dropped as noise) don't depend on which chunk makes them.
- Where a fine chunk meets a coarse one, the coarse source's values are linear along the
  boundary, so the fine lattice's points there lie on the coarse edge. The crossing is the
  same up to rounding, a few centimeters at most.

`topo seams` checks all of this on real output: every endpoint on a boundary between two
chunks needs a partner on the other side. It reports exact matches, near matches (expected
only where resolutions meet), unmatched ends (a bug) and interior ends (missing data in a DEM).

There's a 1 arc second gap at the antimeridian, since lines aren't carried across it.

## The OSM file

Each line is a way, with no OSM ID collisions as long as IDs start above OSM's, which `append`
checks:

| Tag | Metric | Imperial |
|---|---|---|
| `contour` | `elevation` | `elevation` |
| `contour:unit` | `m` | `ft` |
| `ele` | meters | meters, to the centimeter |
| `ele:ft` | | feet |

Lines run with uphill on their left, though merging in planetiler doesn't preserve it.

Planetiler reads nodes, then ways, then relations, each ascending by ID, so `append` writes
the planet's nodes, the contours' nodes, the planet's ways, the contours' ways and the planet's
relations. Blocks are copied without being decoded, and it needs a file marked
`Sort.Type_then_ID`, as planet files and Geofabrik's extracts are.

## Data credit

[JAXA's AW3D30 page](https://www.eorc.jaxa.jp/ALOS/en/dataset/aw3d30/aw3d30_e.htm) allows
commercial and non-commercial use at no charge, under its
[terms](https://earth.jaxa.jp/policy/en.html). Maps need to credit JAXA; check those terms for
the wording to use. Other DEM sources have their own terms.
