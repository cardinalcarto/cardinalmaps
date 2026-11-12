-- What `export` needs to publish each diff's changes after it's applied.
-- Every statement here is quick on a planet database: adding a nullable
-- column, or one with a constant default, doesn't rewrite the table.

-- The replication sequence that added each POI version; NULL for the planet's.
ALTER TABLE poi_versions ADD COLUMN sequence bigint;
CREATE INDEX poi_versions_sequence ON poi_versions (sequence) WHERE sequence IS NOT NULL;

-- Each diff's new `pois` rows, as it wrote them, and each element's identity
-- before the diff (NULL for an element the diff made a POI for the first
-- time). `pois` keeps only the latest row, so this is how an export of an
-- older diff finds what it changed.
CREATE TABLE poi_changes (
    sequence bigint NOT NULL,
    osm_type osm_type NOT NULL,
    osm_id bigint NOT NULL,
    version integer NOT NULL,
    poi_id bigint NOT NULL,
    active boolean NOT NULL,
    tags jsonb NOT NULL,
    previous_poi_id bigint,
    PRIMARY KEY (sequence, osm_type, osm_id)
);

-- Whether the checkpoint's changes are in `poi_changes` and
-- `poi_versions.sequence`. False for the planet, which has no changes, and
-- for diffs applied before this migration, which can't be exported.
ALTER TABLE checkpoints ADD COLUMN journaled boolean NOT NULL DEFAULT false;
