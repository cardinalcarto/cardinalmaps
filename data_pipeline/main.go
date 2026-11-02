// Builds Cardinal's data from an OSM extract: OpenMapTiles-schema vector
// tilesets in MLT format with the local planetiler-openmaptiles fork, the
// airmail geocoder index, and airmail's per-language categorical search
// models. POIs in the tiles and the index are tagged with stable identifiers
// from a poi_service database first.
//
// The engine runs in rootless podman, so select it explicitly:
//
//	DAGGER_ENGINE=image+podman://registry.dagger.io/engine:v1.0.0-beta.21 \
//	  dagger -m data_pipeline api call build --osm=./monaco.osm.pbf \
//	  --wof-db=./whosonfirst-data-admin-latest.spatial.db \
//	  --database-url=env://DATABASE_URL -o out
package main

import (
	"context"
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
	}
}

// Builds tiles, the geocoder index and the categorical search models from
// one OSM extract, into a directory holding tiles.mbtiles, index/ and
// categories/. The steps run one after another, since each can use most of
// a planet build machine's memory.
// +cache="never"
func (m *DataPipeline) Build(
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
) (*dagger.Directory, error) {
	patched := osm
	if databaseUrl != nil {
		var err error
		if patched, err = m.Patch(ctx, osm, databaseUrl, database); err != nil {
			return nil, err
		}
	}
	tiles, err := m.tiles(ctx, patched, args, maxRamPercentage)
	if err != nil {
		return nil, err
	}
	index, err := m.index(ctx, patched, wofDb, pipTree, qrank)
	if err != nil {
		return nil, err
	}
	// The models don't use identifiers, so they're trained from the
	// unpatched extract and don't need retraining when the database changes.
	categories, err := m.CategoricalModels(ctx, osm, wofDb, pipTree, 50_000, 1_000)
	if err != nil {
		return nil, err
	}
	return dag.Directory().
		WithFile("tiles.mbtiles", tiles).
		WithDirectory("index", index).
		WithDirectory("categories", categories), nil
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
) (*dagger.File, error) {
	if databaseUrl != nil {
		var err error
		if osm, err = m.Patch(ctx, osm, databaseUrl, database); err != nil {
			return nil, err
		}
	}
	return m.tiles(ctx, osm, args, maxRamPercentage)
}

func (m *DataPipeline) tiles(ctx context.Context, osm *dagger.File, args []string, maxRamPercentage int) (*dagger.File, error) {
	cmd := []string{
		"java", "-jar", "/app/planetiler-openmaptiles.jar",
		"--osm-path=/input/input.osm.pbf",
		"--download-dir=/data/sources",
		"--tmpdir=/data/tmp",
		"--download",
		"--tile-format=mlt",
		"--output=/out/output.mbtiles",
	}
	cmd = append(cmd, args...)
	ctr, err := m.Planetiler(maxRamPercentage).
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
// Languages are trained one after another, each cached separately.
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
