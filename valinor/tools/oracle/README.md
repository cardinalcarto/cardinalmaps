# Mainline Valhalla oracle

Tools for producing reference data with mainline Valhalla,
which Valinor is tested against.
Everything runs in the pinned container image named in [`valhalla-image`](valhalla-image)
(currently Valhalla 3.9.1), via rootless podman (or `CONTAINER=docker`).

## Tile builds

[`build-tiles.sh`](build-tiles.sh) builds a tile set the same way the Cardinal pipeline does:
admins, timezones, elevation, pedestrian areas, and bounding circles,
plus a tarball extract with a live traffic skeleton.

Landmarks are left out for now.
In 3.9.1, running `valhalla_add_landmarks` produces tiles that route incorrectly:
on the Andorra extract, Loki snaps a location in Andorra la Vella to an edge 5.4 km away
and an 8.07 km route comes back as 0.049 km.
Leaving out only that step fixes it;
pedestrian areas, elevation, and bounding circles are not involved.

```shell
tools/oracle/build-tiles.sh tiles <pbf> <out-dir> <minx,miny,maxx,maxy>
tools/oracle/build-tiles.sh predicted <out-dir> <csv-dir>
```

## Graph tile fixtures

[`make-graphtile-fixtures.sh`](make-graphtile-fixtures.sh) regenerates
`valhalla-graphtile/fixtures` from the pinned Andorra extract in `fixtures/source`
(checked against a SHA-256 before use).

* `andorra-tiles` and `andorra-tiles.tar` are the tile set.
* `andorra-tiles-with-traffic` is tile `0/003/015` after `valhalla_add_predicted_traffic`
  with the rows in [`predicted-speeds`](predicted-speeds),
  which match the `add_predicted_speeds` builder test.
* `andorra-traffic.tar` is the live traffic extract
  with edge `0/3015/42` set to 32 kph by [`set-live-speed.py`](set-live-speed.py).
  That script writes the mainline layout directly so the fixture does not depend on Valinor.

Note that adding predicted speeds changes the tileset build id in *every* tile header,
since mainline derives it from all tile hashes in the set.

The source extract is OpenStreetMap data from Geofabrik,
© OpenStreetMap contributors, available under the Open Database License.
