// Builds Cardinal's data from an OSM extract: OpenMapTiles-schema vector
// tilesets in MLT format with the local planetiler-openmaptiles fork, and the
// airmail geocoder index. POIs in the tiles and the index are tagged with
// stable identifiers from a poi_service database first. A release also holds
// airmail's per-language categorical search models and its query tagger,
// which are trained by hand, rarely; see CategoricalModels. Separately,
// Cardinal Topo's tileset adds contour lines from DEMs; see TopoTiles.
//
// The engine runs in rootless podman, so select it explicitly:
//
//	DAGGER_ENGINE=image+podman://registry.dagger.io/engine:v1.0.0-beta.21 \
//	  dagger -m data_pipeline api call build --osm=./monaco.osm.pbf \
//	  --wof-db=./whosonfirst-data-admin-latest.spatial.db \
//	  --categories=./categories --tagger=./tagger.model \
//	  --database-url=env://DATABASE_URL -o out
package main

import (
	"context"
	_ "embed"
	"fmt"
	"strings"
	"time"

	"dagger/data-pipeline/internal/dagger"
)

const (
	buildImage        = "maven:3.9-eclipse-temurin-25"
	runtimeImage      = "eclipse-temurin:25-jre"
	rustImage         = "rust:1.98-trixie"
	poiServiceRuntime = "debian:trixie-slim"
	airmailRuntime    = "debian:trixie-slim"
	// fasttext has no wheel for Python 3.12, so this needs a compiler.
	uvImage = "ghcr.io/astral-sh/uv:python3.12-trixie"
)

type DataPipeline struct {
	// +private
	Source *dagger.Directory
	// +private
	PoiServiceSource *dagger.Directory
	// +private
	AirmailSource *dagger.Directory
	// +private
	TopoSource *dagger.Directory
	// The pinned mainline Valhalla image, shared with Valinor's oracle.
	// +private
	ValhallaImageRef *dagger.File
}

func New(ws *dagger.Workspace) *DataPipeline {
	return &DataPipeline{
		Source: ws.Directory("/planetiler-openmaptiles", dagger.WorkspaceDirectoryOpts{
			Exclude: []string{".git", ".idea", ".vscode", "target", "data"},
		}),
		PoiServiceSource: ws.Directory("/poi_service", dagger.WorkspaceDirectoryOpts{
			Exclude: []string{"target", "stable_ids"},
		}),
		AirmailSource: ws.Directory("/airmail", dagger.WorkspaceDirectoryOpts{
			Exclude: []string{"target", "data", "categorical/data", "categorical/.venv", "**/__pycache__"},
		}),
		TopoSource: ws.Directory("/topo_pipeline", dagger.WorkspaceDirectoryOpts{
			Exclude: []string{"target"},
		}),
		ValhallaImageRef: ws.File("/valinor/tools/oracle/valhalla-image"),
	}
}

// Builds tiles, the geocoder index and routing tiles from one OSM extract,
// into a release: a directory holding tiles.mbtiles, index/, routing/ (see
// RoutingTiles), and the categorical models and query tagger it's given, as
// categories/ and tagger.model. The steps run one after another, since each
// can use most of a planet build machine's memory.
// +cache="never"
func (m *DataPipeline) Build(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
	// Who's On First SpatiaLite database covering the extract.
	wofDb *dagger.File,
	// Categorical search models from CategoricalModels, usually the last
	// release's: how people describe POIs changes slowly, so they're retrained
	// by hand.
	categories *dagger.Directory,
	// A query tagger model (airmail's `tagger train`), usually the last
	// release's, for the same reason.
	tagger *dagger.File,
	// PostgreSQL URL of the poi_service database that gives POIs their
	// identifiers. Without it, POIs have none.
	// +optional
	databaseUrl *dagger.Secret,
	// The database, if it isn't otherwise reachable; see Patch.
	// +optional
	database *dagger.Service,
	// Extra planetiler arguments, e.g. --exclude-layers=poi or --mlt-advanced.
	// +optional
	args []string,
	// +default=50
	maxRamPercentage int,
	// Use an in-memory tree of Who's On First polygons for point-in-polygon
	// lookups; see PipTree on Index.
	// +default=true
	pipTree bool,
	// QRank (qrank.csv.gz from https://qrank.wmcloud.org), which ranks POIs
	// with Wikidata entities by their articles' page views. Without it,
	// having an entity counts the same for every POI.
	// +optional
	qrank *dagger.File,
	// A directory holding planetiler's auxiliary sources under their usual
	// names (water-polygons-split-3857.zip, natural_earth_vector.sqlite.zip,
	// lake_centerline.shp.zip), to use instead of downloading them.
	// +optional
	sources *dagger.Directory,
	// A configuration for contour lines, like topo_pipeline/topo.example.toml,
	// to include them in the tiles. Only the tiles get them.
	// +optional
	contourConfig *dagger.File,
	// DEM rasters, for a configuration that reads them from /dem; see Contours.
	// +optional
	dem *dagger.Directory,
	// Credentials, for a configuration that reads DEMs from a bucket.
	// +optional
	s3AccessKeyId *dagger.Secret,
	// +optional
	s3SecretAccessKey *dagger.Secret,
	// Contours already made with Contours, instead of making them from
	// contourConfig and dem. A planet's DEMs are hundreds of gigabytes, which
	// needn't stay in the engine for the rest of the build.
	// +optional
	contours *dagger.File,
	// Default speeds for the routing tiles; see RoutingTiles.
	// +optional
	routingSpeeds *dagger.File,
	// GEDTM, for elevation in the routing tiles; see RoutingTiles.
	// +optional
	gedtm string,
	// The server GEDTM is read from, if it's served from this machine; see
	// RoutingTiles.
	// +optional
	gedtmService *dagger.Service,
) (*dagger.Directory, error) {
	patched := osm
	if databaseUrl != nil {
		var err error
		if patched, err = m.Patch(ctx, osm, databaseUrl, database); err != nil {
			return nil, err
		}
	}
	forTiles := patched
	if contours != nil && contourConfig != nil {
		return nil, fmt.Errorf("pass contours or contourConfig, not both")
	}
	if contourConfig != nil {
		var err error
		if contours, err = m.Contours(ctx, dem, contourConfig, "", s3AccessKeyId, s3SecretAccessKey); err != nil {
			return nil, err
		}
	}
	if contours != nil {
		var err error
		if forTiles, err = m.AppendContours(ctx, patched, contours); err != nil {
			return nil, err
		}
		args = append(topoTileArgs(), args...)
	}
	tiles, err := m.tiles(ctx, forTiles, wofDb, args, maxRamPercentage, sources)
	if err != nil {
		return nil, err
	}
	index, err := m.index(ctx, patched, wofDb, pipTree, qrank)
	if err != nil {
		return nil, err
	}
	// Routing doesn't use POI identifiers, so it reads the extract itself,
	// and its tiles can be reused when only the POI database has changed.
	routing, err := m.RoutingTiles(ctx, osm, routingSpeeds, gedtm, gedtmService)
	if err != nil {
		return nil, err
	}
	return dag.Directory().
		WithFile("tiles.mbtiles", tiles).
		WithDirectory("index", index).
		WithDirectory("routing", routing).
		WithDirectory("categories", categories).
		WithFile("tagger.model", tagger), nil
}

// The poi_service executable.
func (m *DataPipeline) PoiService() *dagger.File {
	return dag.Container().
		From(rustImage).
		WithDirectory("/src", m.PoiServiceSource).
		WithMountedCache("/usr/local/cargo/registry", dag.CacheVolume("poi-service-cargo-registry")).
		WithMountedCache("/src/target", dag.CacheVolume("poi-service-target")).
		WithWorkdir("/src").
		WithExec([]string{"cargo", "build", "--release", "--locked"}).
		WithExec([]string{"cp", "target/release/poi_service", "/poi_service"}).
		File("/poi_service")
}

// Copies an OSM extract, tagging every POI version that the poi_service
// database knows with its stable identifier (cardinal:poi_id) and OSM
// version (cardinal:version).
// +cache="never"
func (m *DataPipeline) Patch(
	ctx context.Context,
	// OSM extract (.osm.pbf) with element metadata.
	osm *dagger.File,
	// PostgreSQL URL of the poi_service database.
	databaseUrl *dagger.Secret,
	// The database, if it isn't otherwise reachable from the build, e.g.
	// tcp://localhost:5432 for one on this machine. It's reachable at host
	// "database", so use that in --database-url.
	// +optional
	database *dagger.Service,
) (*dagger.File, error) {
	ctr := dag.Container().
		From(poiServiceRuntime).
		WithFile("/usr/local/bin/poi_service", m.PoiService()).
		WithSecretVariable("DATABASE_URL", databaseUrl)
	if database != nil {
		ctr = ctr.WithServiceBinding("database", database)
	}
	// The database changes as diffs are applied, which Dagger can't see, so
	// the patch is keyed on its latest checkpoint and redone only when that
	// changes.
	checkpoint, err := ctr.
		WithEnvVariable("CACHE_BUSTER", time.Now().String()).
		WithExec([]string{"poi_service", "checkpoint"}).
		Stdout(ctx)
	if err != nil {
		return nil, err
	}
	ctr, err = ctr.
		WithEnvVariable("POI_SERVICE_CHECKPOINT", strings.TrimSpace(checkpoint)).
		WithMountedFile("/input/input.osm.pbf", osm).
		WithExec([]string{"mkdir", "-p", "/out"}).
		WithExec([]string{"poi_service", "patch", "--input=/input/input.osm.pbf", "--output=/out/output.osm.pbf"}).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	return ctr.File("/out/output.osm.pbf"), nil
}

// The planetiler-openmaptiles fork's executable jar with dependencies.
func (m *DataPipeline) Jar() *dagger.File {
	return dag.Container().
		From(buildImage).
		WithMountedCache("/root/.m2", dag.CacheVolume("planetiler-openmaptiles-m2")).
		WithDirectory("/src", m.Source).
		WithWorkdir("/src").
		WithExec([]string{"mvn", "-B", "-DskipTests", "-Dmaven.buildNumber.skip=true", "package"}).
		WithExec([]string{"sh", "-c", "cp target/*-with-deps.jar /planetiler-openmaptiles.jar"}).
		File("/planetiler-openmaptiles.jar")
}

// A container ready to run the fork, with the auxiliary sources (Natural
// Earth, water polygons, lake centerlines) kept in a cache volume.
func (m *DataPipeline) Planetiler(
	// Share of the engine's memory given to the JVM heap. Rootless podman
	// usually puts no cgroup limit on the engine, so this is the host's RAM.
	// +default=50
	maxRamPercentage int,
) *dagger.Container {
	return dag.Container().
		From(runtimeImage).
		WithFile("/app/planetiler-openmaptiles.jar", m.Jar()).
		WithMountedCache("/data/sources", dag.CacheVolume("planetiler-openmaptiles-sources"), dagger.ContainerWithMountedCacheOpts{
			// Downloads aren't atomic; keep concurrent builds from seeing partial files.
			Sharing: dagger.CacheSharingModeLocked,
		}).
		WithMountedCache("/data/tmp", dag.CacheVolume("planetiler-openmaptiles-tmp"), dagger.ContainerWithMountedCacheOpts{
			Sharing: dagger.CacheSharingModePrivate,
		}).
		WithWorkdir("/").
		WithEnvVariable("JAVA_TOOL_OPTIONS", fmt.Sprintf("-XX:MaxRAMPercentage=%d", maxRamPercentage))
}

// Builds an MLT mbtiles tileset from a local OSM extract.
// +cache="never"
func (m *DataPipeline) Tiles(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
	// PostgreSQL URL of the poi_service database that gives POIs their
	// identifiers. Without it, POIs have no feature IDs.
	// +optional
	databaseUrl *dagger.Secret,
	// The database, if it isn't otherwise reachable; see Patch.
	// +optional
	database *dagger.Service,
	// Extra planetiler arguments, e.g. --exclude-layers=poi or --mlt-advanced.
	// +optional
	args []string,
	// +default=50
	maxRamPercentage int,
	// A directory holding planetiler's auxiliary sources under their usual
	// names (water-polygons-split-3857.zip, natural_earth_vector.sqlite.zip,
	// lake_centerline.shp.zip), to use instead of downloading them.
	// +optional
	sources *dagger.Directory,
	// Who's On First SpatiaLite database covering the extract, to give POIs
	// the areas they're in.
	// +optional
	wofDb *dagger.File,
) (*dagger.File, error) {
	if databaseUrl != nil {
		var err error
		if osm, err = m.Patch(ctx, osm, databaseUrl, database); err != nil {
			return nil, err
		}
	}
	return m.tiles(ctx, osm, wofDb, args, maxRamPercentage, sources)
}

func (m *DataPipeline) tiles(ctx context.Context, osm *dagger.File, wofDb *dagger.File, args []string, maxRamPercentage int, sources *dagger.Directory) (*dagger.File, error) {
	cmd := []string{
		"java", "-jar", "/app/planetiler-openmaptiles.jar",
		"--osm-path=/input/input.osm.pbf",
		"--download-dir=/data/sources",
		"--tmpdir=/data/tmp",
		"--download",
		"--tile-format=mlt",
		"--output=/out/output.mbtiles",
	}
	planetiler := m.Planetiler(maxRamPercentage)
	if sources != nil {
		planetiler = planetiler.WithMountedDirectory("/input/sources", sources)
		cmd = append(cmd, "--download=false",
			"--water_polygons_path=/input/sources/water-polygons-split-3857.zip",
			"--natural_earth_path=/input/sources/natural_earth_vector.sqlite.zip",
			"--lake_centerlines_path=/input/sources/lake_centerline.shp.zip")
	}
	if wofDb != nil {
		// POIs get the areas they're in.
		planetiler = planetiler.WithMountedFile("/input/wof.spatial.db", wofDb)
		cmd = append(cmd, "--wof_db=/input/wof.spatial.db")
	}
	cmd = append(cmd, args...)
	ctr, err := planetiler.
		WithMountedFile("/input/input.osm.pbf", osm).
		// Clear temp files left by an interrupted run.
		WithExec([]string{"sh", "-c", "rm -rf /data/tmp/* && mkdir -p /out"}).
		WithExec(cmd).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	return ctr.File("/out/output.mbtiles"), nil
}

// The topo_pipeline executable.
func (m *DataPipeline) Topo() *dagger.File {
	return dag.Container().
		From(rustImage).
		WithDirectory("/src", m.TopoSource).
		WithMountedCache("/usr/local/cargo/registry", dag.CacheVolume("topo-cargo-registry")).
		WithMountedCache("/src/target", dag.CacheVolume("topo-target")).
		WithWorkdir("/src").
		WithExec([]string{"cargo", "build", "--release", "--locked"}).
		WithExec([]string{"cp", "target/release/topo", "/topo"}).
		File("/topo")
}

// Generates contour lines in both metric and imperial as a sorted .osm.pbf.
// Finished chunks are kept in a cache volume, so adding a higher resolution
// DEM to the configuration recomputes only the chunks it touches.
// +cache="never"
func (m *DataPipeline) Contours(
	ctx context.Context,
	// DEM rasters, mounted at /dem; see topo_pipeline/README.md for what they
	// must be. The configuration's source directories start with /dem. Not
	// needed for sources read from a bucket.
	// +optional
	dem *dagger.Directory,
	// The configuration, like topo_pipeline/topo.example.toml.
	config *dagger.File,
	// west,south,east,north, to limit the region.
	// +optional
	bbox string,
	// Credentials, for sources read from a bucket.
	// +optional
	s3AccessKeyId *dagger.Secret,
	// +optional
	s3SecretAccessKey *dagger.Secret,
) (*dagger.File, error) {
	cmd := []string{"/usr/local/bin/topo", "contours", "--config=/input/topo.toml", "--work=/work", "--out=/out/contours.osm.pbf"}
	if bbox != "" {
		cmd = append(cmd, "--bbox="+bbox)
	}
	base := dag.Container().From(poiServiceRuntime)
	if dem != nil {
		base = base.WithMountedDirectory("/dem", dem)
	}
	if s3AccessKeyId != nil && s3SecretAccessKey != nil {
		base = base.
			WithSecretVariable("AWS_ACCESS_KEY_ID", s3AccessKeyId).
			WithSecretVariable("AWS_SECRET_ACCESS_KEY", s3SecretAccessKey)
	}
	ctr, err := base.
		WithFile("/usr/local/bin/topo", m.Topo()).
		WithMountedFile("/input/topo.toml", config).
		WithMountedCache("/work", dag.CacheVolume("topo-chunks")).
		WithExec([]string{"mkdir", "-p", "/out"}).
		WithExec(cmd).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	return ctr.File("/out/contours.osm.pbf"), nil
}

// Copies an OSM extract with contours spliced in, in the order planetiler
// needs: nodes, then ways, then relations, each ascending by ID.
// +cache="never"
func (m *DataPipeline) AppendContours(
	ctx context.Context,
	// OSM extract (.osm.pbf), sorted.
	osm *dagger.File,
	// Contours from Contours.
	contours *dagger.File,
) (*dagger.File, error) {
	ctr, err := dag.Container().
		From(poiServiceRuntime).
		WithFile("/usr/local/bin/topo", m.Topo()).
		WithMountedFile("/input/input.osm.pbf", osm).
		WithMountedFile("/input/contours.osm.pbf", contours).
		WithExec([]string{"mkdir", "-p", "/out"}).
		WithExec([]string{"/usr/local/bin/topo", "append", "--planet=/input/input.osm.pbf", "--contours=/input/contours.osm.pbf", "--out=/out/output.osm.pbf"}).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	return ctr.File("/out/output.osm.pbf"), nil
}

// Copies an OSM extract with each way carrying its nodes' locations (osmium
// add-locations-to-ways), as airmail's indexer and categories_export read it.
// Untagged nodes other than relation members are dropped, so it's not for
// tiles: planetiler reads every node.
func (m *DataPipeline) LocationsOnWays(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
) (*dagger.File, error) {
	size, err := osm.Size(ctx)
	if err != nil {
		return nil, err
	}
	// osmium's default index keeps every location in memory, which suits an
	// extract. The planet's is about 110GB, so it goes on disk instead, where
	// it's read through the page cache.
	index := "flex_mem"
	if size > 20<<30 {
		index = "dense_file_array,/work/nodes.idx"
	}
	ctr, err := dag.Container().
		From(poiServiceRuntime).
		WithExec([]string{"sh", "-c", "apt-get update && apt-get install -y --no-install-recommends osmium-tool && rm -rf /var/lib/apt/lists/*"}).
		WithMountedFile("/input/input.osm.pbf", osm).
		WithMountedCache("/work", dag.CacheVolume("osmium-node-index"), dagger.ContainerWithMountedCacheOpts{
			Sharing: dagger.CacheSharingModePrivate,
		}).
		WithExec([]string{"sh", "-c", "rm -f /work/nodes.idx && mkdir -p /out"}).
		WithExec([]string{"osmium", "add-locations-to-ways", "--keep-member-nodes", "-i", index,
			"-o", "/out/output.osm.pbf", "/input/input.osm.pbf"}).
		WithExec([]string{"rm", "-f", "/work/nodes.idx"}).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	return ctr.File("/out/output.osm.pbf"), nil
}

// Builds the Cardinal Topo tileset: the OpenMapTiles layers, plus contour_m
// and contour_ft, from an OSM extract and DEMs.
// +cache="never"
func (m *DataPipeline) TopoTiles(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
	// DEM rasters; see Contours.
	// +optional
	dem *dagger.Directory,
	// The configuration; see Contours.
	config *dagger.File,
	// west,south,east,north, to limit the contours' region.
	// +optional
	bbox string,
	// Extra planetiler arguments.
	// +optional
	args []string,
	// +default=50
	maxRamPercentage int,
	// A directory holding planetiler's auxiliary sources under their usual
	// names (water-polygons-split-3857.zip, natural_earth_vector.sqlite.zip,
	// lake_centerline.shp.zip), to use instead of downloading them.
	// +optional
	sources *dagger.Directory,
	// Credentials, for sources read from a bucket; see Contours.
	// +optional
	s3AccessKeyId *dagger.Secret,
	// +optional
	s3SecretAccessKey *dagger.Secret,
) (*dagger.File, error) {
	contours, err := m.Contours(ctx, dem, config, bbox, s3AccessKeyId, s3SecretAccessKey)
	if err != nil {
		return nil, err
	}
	combined, err := m.AppendContours(ctx, osm, contours)
	if err != nil {
		return nil, err
	}
	return m.tiles(ctx, combined, nil, append(topoTileArgs(), args...), maxRamPercentage, sources)
}

// What the Cardinal Topo tileset needs from planetiler beyond the defaults.
func topoTileArgs() []string {
	return []string{"--transportation_z13_paths=true", "--waterway_z12_streams=true"}
}

// A container with airmail's indexer and categories_export executables.
func (m *DataPipeline) AirmailIndexer() *dagger.Container {
	build := dag.Container().
		From(rustImage).
		WithExec([]string{"sh", "-c", "apt-get update && apt-get install -y --no-install-recommends clang pkg-config libzstd-dev"}).
		WithDirectory("/src", m.AirmailSource).
		WithMountedCache("/usr/local/cargo/registry", dag.CacheVolume("airmail-cargo-registry")).
		WithMountedCache("/src/target", dag.CacheVolume("airmail-target")).
		WithWorkdir("/src").
		WithExec([]string{"cargo", "build", "--release", "--locked", "-p", "airmail_indexer", "--bin", "indexer", "--bin", "categories_export"}).
		WithExec([]string{"sh", "-c", "mkdir -p /out && cp target/release/indexer target/release/categories_export /out/"})
	return dag.Container().
		From(airmailRuntime).
		// The indexer loads SpatiaLite to read the Who's On First database.
		WithExec([]string{"sh", "-c", "apt-get update && apt-get install -y --no-install-recommends libsqlite3-mod-spatialite && rm -rf /var/lib/apt/lists/*"}).
		WithFile("/usr/local/bin/indexer", build.File("/out/indexer")).
		WithFile("/usr/local/bin/categories_export", build.File("/out/categories_export"))
}

// Mounts a Who's On First database, returning the flags that point airmail
// at it and at the caches derived from it. The caches are kept in a cache
// volume named by the database's digest, since Dagger doesn't key cache
// volumes on what went into them. Once a database is replaced, nothing mounts
// its volume, so the engine's garbage collection can reclaim it.
func (m *DataPipeline) withWof(ctx context.Context, ctr *dagger.Container, wofDb *dagger.File, pipTree bool) (*dagger.Container, []string, error) {
	digest, err := wofDb.Digest(ctx)
	if err != nil {
		return nil, nil, err
	}
	volume := "airmail-wof-" + strings.ReplaceAll(digest, ":", "-")
	ctr = ctr.
		WithMountedFile("/input/wof.spatial.db", wofDb).
		// The admin cache is a redb file that only one process can open.
		WithMountedCache("/cache", dag.CacheVolume(volume), dagger.ContainerWithMountedCacheOpts{
			Sharing: dagger.CacheSharingModeLocked,
		})
	flags := []string{"--wof-db=/input/wof.spatial.db", "--admin-cache=/cache/admin-cache"}
	if pipTree {
		flags = append(flags, "--pip-tree=/cache/pip-tree.bin")
	}
	return ctr, flags, nil
}

// Builds an airmail geocoder index from an OSM extract.
// +cache="never"
func (m *DataPipeline) Index(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
	// Who's On First SpatiaLite database covering the extract.
	wofDb *dagger.File,
	// PostgreSQL URL of the poi_service database that gives POIs their
	// identifiers. Without it, POIs have none.
	// +optional
	databaseUrl *dagger.Secret,
	// The database, if it isn't otherwise reachable; see Patch.
	// +optional
	database *dagger.Service,
	// Use an in-memory tree of Who's On First polygons for point-in-polygon
	// lookups instead of SpatiaLite. It's much faster for the planet, but
	// uses around 10GB of memory and takes minutes to build the first time.
	// +default=true
	pipTree bool,
	// QRank (qrank.csv.gz from https://qrank.wmcloud.org), which ranks POIs
	// with Wikidata entities by their articles' page views. Without it,
	// having an entity counts the same for every POI.
	// +optional
	qrank *dagger.File,
) (*dagger.Directory, error) {
	if databaseUrl != nil {
		var err error
		if osm, err = m.Patch(ctx, osm, databaseUrl, database); err != nil {
			return nil, err
		}
	}
	return m.index(ctx, osm, wofDb, pipTree, qrank)
}

// Indexes the extract's POIs and addresses, then every administrative area
// in the Who's On First database.
func (m *DataPipeline) index(ctx context.Context, osm *dagger.File, wofDb *dagger.File, pipTree bool, qrank *dagger.File) (*dagger.Directory, error) {
	ctr, flags, err := m.withWof(ctx, m.AirmailIndexer(), wofDb, pipTree)
	if err != nil {
		return nil, err
	}
	if qrank != nil {
		ctr = ctr.WithMountedFile("/input/qrank.csv.gz", qrank)
		flags = append(flags, "--qrank=/input/qrank.csv.gz")
	}
	indexer := append([]string{"indexer", "--index=/out/index"}, flags...)
	osm, err = m.LocationsOnWays(ctx, osm)
	if err != nil {
		return nil, err
	}
	ctr, err = ctr.
		WithMountedFile("/input/input.osm.pbf", osm).
		WithExec([]string{"mkdir", "-p", "/out/index"}).
		WithExec(append(append([]string{}, indexer...), "load-osm-pbf", "/input/input.osm.pbf")).
		WithExec(append(append([]string{}, indexer...), "load-wof")).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	return ctr.Directory("/out/index"), nil
}

// Trains airmail's categorical search models from an OSM extract, into a
// directory of <language>.amcats files. See airmail/categorical/README.md.
// Languages are trained one after another, each cached separately. This is
// run by hand, now and then; Build takes the models it makes.
func (m *DataPipeline) CategoricalModels(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
	// Who's On First SpatiaLite database covering the extract, for the
	// languages spoken where each POI is.
	wofDb *dagger.File,
	// See Index.
	// +default=true
	pipTree bool,
	// The fewest POIs a language needs for a model.
	// +default=50000
	minPois int,
	// The fewest cuisine-tagged POIs a language needs for a model.
	// +default=1000
	minCuisine int,
) (*dagger.Directory, error) {
	ctr, flags, err := m.withWof(ctx, m.AirmailIndexer(), wofDb, pipTree)
	if err != nil {
		return nil, err
	}
	cmd := append([]string{"categories_export"}, flags...)
	cmd = append(cmd, "/input/input.osm.pbf", "/out/export.jsonl")
	if osm, err = m.LocationsOnWays(ctx, osm); err != nil {
		return nil, err
	}
	ctr, err = ctr.
		WithMountedFile("/input/input.osm.pbf", osm).
		WithExec([]string{"mkdir", "-p", "/out"}).
		WithExec(cmd).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	export := ctr.File("/out/export.jsonl")

	python := dag.Container().
		From(uvImage).
		WithDirectory("/src", m.AirmailSource).
		WithWorkdir("/src/categorical").
		WithMountedCache("/root/.cache/uv", dag.CacheVolume("airmail-categorical-uv")).
		// The uv cache is on another filesystem, so it can't be hard-linked.
		WithEnvVariable("UV_LINK_MODE", "copy").
		WithExec([]string{"uv", "sync", "--frozen"}).
		WithExec([]string{"mkdir", "-p", "/work", "/out"})
	augment := python.
		WithExec([]string{"uv", "run", "--frozen", "categories-augment", "/work/augment.jsonl", "--cache=/work"}).
		File("/work/augment.jsonl")
	languages, err := python.
		WithMountedFile("/input/export.jsonl", export).
		WithMountedFile("/input/augment.jsonl", augment).
		WithExec([]string{
			"uv", "run", "--frozen", "categories-languages", "/input/export.jsonl", "/input/augment.jsonl", "/out/languages",
			fmt.Sprintf("--min-pois=%d", minPois), fmt.Sprintf("--min-cuisine=%d", minCuisine),
		}).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	qualifying, err := languages.File("/out/languages/qualifying.txt").Contents(ctx)
	if err != nil {
		return nil, err
	}

	models := dag.Directory()
	for _, language := range strings.Fields(qualifying) {
		model := language + ".amcats"
		ctr, err := python.
			WithMountedFile("/input/pois.jsonl", languages.File("/out/languages/"+language+".jsonl")).
			WithMountedFile("/input/augment.jsonl", augment).
			WithExec([]string{
				"uv", "run", "--frozen", "categories-train", "/input/pois.jsonl", "/input/augment.jsonl", "/out/" + model,
				"--language=" + language, "--work=/work/train",
			}).
			Sync(ctx)
		if err != nil {
			return nil, fmt.Errorf("training %s: %w", language, err)
		}
		models = models.WithFile(model, ctr.File("/out/"+model))
	}
	return models, nil
}

// The pinned mainline Valhalla image that routing tiles are built with, as
// valinor/tools/oracle/valhalla-image names it (a tag and digest), so the
// pipeline and Valinor's oracle build tiles with the same tools.
func (m *DataPipeline) ValhallaImage(ctx context.Context) (string, error) {
	ref, err := m.ValhallaImageRef.Contents(ctx)
	if err != nil {
		return "", err
	}
	ref = strings.TrimSpace(ref)
	if !strings.Contains(ref, "@sha256:") {
		return "", fmt.Errorf("valinor/tools/oracle/valhalla-image must pin a digest, not %q", ref)
	}
	return ref, nil
}

// Valhalla's time zone database (valhalla_build_timezones), from the
// timezone-boundary-builder release that the image's script names. It doesn't
// depend on the extract, so it's built once per image.
func (m *DataPipeline) RoutingTimezones(ctx context.Context) (*dagger.File, error) {
	image, err := m.ValhallaImage(ctx)
	if err != nil {
		return nil, err
	}
	return dag.Container().
		From(image).
		WithWorkdir("/work").
		WithExec([]string{"sh", "-c", "mkdir -p /out && valhalla_build_timezones >/out/timezones.sqlite"}).
		File("/out/timezones.sqlite"), nil
}

// Valhalla's administrative areas (valhalla_build_admins) for an extract:
// countries and their subdivisions, which give edges their country's access
// rules and driving side, and choose default speeds by region.
func (m *DataPipeline) RoutingAdmins(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
) (*dagger.File, error) {
	image, err := m.ValhallaImage(ctx)
	if err != nil {
		return nil, err
	}
	return dag.Container().
		From(image).
		WithMountedFile("/input/input.osm.pbf", osm).
		WithWorkdir("/out").
		WithExec([]string{"sh", "-c", "valhalla_build_config --mjolnir-admin /out/admins.sqlite >/tmp/valhalla.json && " +
			"valhalla_build_admins -c /tmp/valhalla.json /input/input.osm.pbf"}).
		File("/out/admins.sqlite"), nil
}

// Builds Valhalla routing tiles from an OSM extract with mainline Valhalla's
// own tile builder (the pinned image; see ValhallaImage), into a directory
// holding tiles.tar (valhalla_build_extract's tarball, with its index) and
// valhalla.json, the configuration to serve it with. In the configuration,
// tile_extract is tiles.tar, relative to the directory, and the paths that
// only the build reads are cleared. Valinor's oracle builds tiles the same
// way: pedestrian areas, bounding circles, admins and time zones, and no
// landmarks, since mainline's landmarks step makes tiles that route
// incorrectly (see valinor/tools/oracle/README.md).
//
// With gedtm, edges get elevation (heights along them, and grades) from
// GEDTM: after the stages before elevation, the 1x1 degree cells under the
// graph tiles are cut from it into .hgt tiles beside them, which the
// elevation stage samples (see routing_elevation.py). GEDTM is read in place,
// with GDAL's range requests, so neither it nor the cut tiles enter the
// engine's store; the tiles are deleted with the rest of the build's work.
// +cache="never"
func (m *DataPipeline) RoutingTiles(
	ctx context.Context,
	// OSM extract (.osm.pbf).
	osm *dagger.File,
	// Default speeds by region, density and road class
	// (mjolnir.default_speeds_config), such as default_speeds.json from
	// https://github.com/OpenStreetMapSpeeds/schema. Without it, edges
	// without a maxspeed get mainline's built-in speeds by road class.
	// +optional
	speeds *dagger.File,
	// OpenGeoHub's GEDTM v1.2, the global 1 arcsecond DTM
	// (gedtm_rf_m_30m_s_20060101_20151231_go_epsg.4326.3855_v1.2.tif), as GDAL
	// opens it from the build's container: /vsicurl/ and a URL serving it
	// with range requests, such as
	// /vsicurl/http://gedtm:8080/gedtm_rf_m_30m_s_20060101_20151231_go_epsg.4326.3855_v1.2.tif
	// with gedtmService. Without it, the tiles have no elevation.
	// +optional
	gedtm string,
	// A server for GEDTM, such as a file server on this machine passed as
	// tcp://localhost:8080, which the build reaches as host gedtm.
	// +optional
	gedtmService *dagger.Service,
) (*dagger.Directory, error) {
	if gedtmService != nil && gedtm == "" {
		return nil, fmt.Errorf("gedtmService needs gedtm, the path to read from it")
	}
	image, err := m.ValhallaImage(ctx)
	if err != nil {
		return nil, err
	}
	timezones, err := m.RoutingTimezones(ctx)
	if err != nil {
		return nil, err
	}
	admins, err := m.RoutingAdmins(ctx, osm)
	if err != nil {
		return nil, err
	}
	config := []string{
		"valhalla_build_config",
		"--mjolnir-tile-dir", "/work/tiles",
		"--mjolnir-tile-extract", "/out/tiles.tar",
		"--mjolnir-traffic-extract", "",
		"--mjolnir-admin", "/input/admins.sqlite",
		"--mjolnir-timezone", "/input/timezones.sqlite",
		"--mjolnir-landmarks", "",
		"--mjolnir-pedestrian-areas", "True",
		"--mjolnir-data-processing-build-bounding-circles", "True",
	}
	ctr := dag.Container().From(image)
	if gedtm != "" {
		config = append(config, "--additional-data-elevation", "/work/elevation")
		// The image has Python but not GDAL.
		ctr = ctr.
			WithEnvVariable("DEBIAN_FRONTEND", "noninteractive").
			WithExec([]string{"sh", "-c", "apt-get update && apt-get install -y --no-install-recommends python3-gdal && rm -rf /var/lib/apt/lists/*"}).
			WithNewFile("/input/routing_elevation.py", routingElevationScript)
		if gedtmService != nil {
			ctr = ctr.WithServiceBinding("gedtm", gedtmService)
		}
	} else {
		config = append(config, "--additional-data-elevation", "")
	}
	ctr = ctr.
		WithMountedFile("/input/input.osm.pbf", osm).
		WithMountedFile("/input/admins.sqlite", admins).
		WithMountedFile("/input/timezones.sqlite", timezones).
		// A planet's tiles are many tens of gigabytes before they're packed
		// into the tarball, so they're built in a cache volume, not a layer.
		WithMountedCache("/work", dag.CacheVolume("routing-tiles-work"), dagger.ContainerWithMountedCacheOpts{
			Sharing: dagger.CacheSharingModePrivate,
		}).
		WithWorkdir("/work")
	if speeds != nil {
		ctr = ctr.WithMountedFile("/input/default_speeds.json", speeds)
		config = append(config, "--mjolnir-default-speeds-config", "/input/default_speeds.json")
	}
	ctr = ctr.
		// Clear what an interrupted run left behind.
		WithExec([]string{"sh", "-c", "rm -rf /work/* && mkdir -p /out /work/tiles"}).
		WithExec(config, dagger.ContainerWithExecOpts{RedirectStdout: "/work/valhalla.json"})
	buildTiles := []string{"valhalla_build_tiles", "-c", "/work/valhalla.json"}
	if gedtm != "" {
		// Elevation is the stage after restrictions; its tiles are cut once
		// the graph tiles show which cells it needs.
		ctr = ctr.
			WithExec(append(buildTiles, "-e", "restrictions", "/input/input.osm.pbf")).
			WithExec([]string{"python3", "-I", "/input/routing_elevation.py", gedtm, "/input/input.osm.pbf", "/work/tiles", "/work/elevation", fmt.Sprint(elevationJobs)}).
			WithExec([]string{"du", "-sh", "/work/elevation"}).
			WithExec(append(buildTiles, "-s", "elevation", "/input/input.osm.pbf"))
	} else {
		ctr = ctr.WithExec(append(buildTiles, "/input/input.osm.pbf"))
	}
	ctr, err = ctr.
		WithExec([]string{"valhalla_build_extract", "-c", "/work/valhalla.json", "--overwrite"}).
		WithExec([]string{"python3", "-I", "-c", routingConfigScript, "/work/valhalla.json", "/out/valhalla.json"}).
		WithExec([]string{"sh", "-c", "rm -rf /work/*"}).
		Sync(ctx)
	if err != nil {
		return nil, err
	}
	return dag.Directory().
		WithFile("tiles.tar", ctr.File("/out/tiles.tar")).
		WithFile("valhalla.json", ctr.File("/out/valhalla.json")), nil
}

// Cuts the elevation tiles that a build needs from GEDTM.
//
//go:embed routing_elevation.py
var routingElevationScript string

// How many cells are cut from GEDTM at once. Each one reads around 50MB of it.
const elevationJobs = 8

// Rewrites the build's valhalla.json into the one served with the tarball:
// tile_extract is the tarball next to it, and the build's inputs are gone.
const routingConfigScript = `
import json, sys
with open(sys.argv[1]) as f:
    config = json.load(f)
mjolnir = config["mjolnir"]
mjolnir["tile_extract"] = "tiles.tar"
for key in ("tile_dir", "traffic_extract", "admin", "timezone", "landmarks", "default_speeds_config"):
    mjolnir[key] = ""
config["additional_data"]["elevation"] = ""
with open(sys.argv[2], "w") as f:
    json.dump(config, f, indent=2)
    f.write("\n")
`
