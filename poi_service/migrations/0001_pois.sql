CREATE TYPE osm_type AS ENUM ('node', 'way', 'relation');

-- Every element that has ever been a POI, as of its latest version.
CREATE TABLE pois (
    osm_type osm_type NOT NULL,
    osm_id bigint NOT NULL,
    -- The element's latest version, which may be deleted or not a POI.
    version integer NOT NULL,
    -- The identity of the element's last POI version.
    poi_id bigint NOT NULL,
    -- Whether the latest version is a POI.
    active boolean NOT NULL,
    -- Tags of the element's last POI version.
    tags jsonb NOT NULL,
    PRIMARY KEY (osm_type, osm_id)
);
CREATE INDEX pois_poi_id ON pois (poi_id);

-- The identity of every POI version seen since the planet.
CREATE TABLE poi_versions (
    osm_type osm_type NOT NULL,
    osm_id bigint NOT NULL,
    version integer NOT NULL,
    poi_id bigint NOT NULL,
    PRIMARY KEY (osm_type, osm_id, version)
);
CREATE INDEX poi_versions_poi_id ON poi_versions (poi_id);

-- What identifiers depend on: deployments with the same planet timestamp and
-- ruleset, applying the same diffs, assign the same identifiers.
CREATE TABLE origin (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    planet_timestamp timestamptz NOT NULL,
    ruleset integer NOT NULL,
    replication_url text NOT NULL
);

-- One row per applied replication sequence; the first is the planet itself.
-- Deployments can compare checksums to confirm they agree.
CREATE TABLE checkpoints (
    sequence bigint PRIMARY KEY,
    timestamp timestamptz NOT NULL,
    -- The identifier the next new identity gets.
    next_poi_id bigint NOT NULL,
    checksum bytea NOT NULL,
    applied_at timestamptz NOT NULL DEFAULT now()
);
