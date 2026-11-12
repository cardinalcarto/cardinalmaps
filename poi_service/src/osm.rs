//! OSM element versions, read from planet files and replication diffs.

use std::io::BufRead;

use anyhow::{Context, Result, bail};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

use crate::rules::{Tags, is_poi};

/// Element types, ordered as in planet files: nodes, then ways, then relations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, sqlx::Type)]
#[sqlx(type_name = "osm_type", rename_all = "lowercase")]
pub enum OsmType {
    Node,
    Way,
    Relation,
}

impl OsmType {
    /// The type with this index (0 node, 1 way, 2 relation).
    pub fn from_index(index: u8) -> Option<OsmType> {
        [OsmType::Node, OsmType::Way, OsmType::Relation].get(usize::from(index)).copied()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OsmType::Node => "node",
            OsmType::Way => "way",
            OsmType::Relation => "relation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementKey {
    pub osm_type: OsmType,
    pub osm_id: i64,
}

/// One version of an element, keeping its tags only if it's a POI.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementVersion {
    pub key: ElementKey,
    pub version: i32,
    /// The version's tags if it is a visible POI; `None` if it was deleted or
    /// isn't a POI.
    pub poi_tags: Option<Tags>,
}

/// Collects tags into a map if they describe a POI.
pub fn poi_tags<'a>(tags: impl Iterator<Item = (&'a str, &'a str)> + Clone) -> Option<Tags> {
    is_poi(tags.clone()).then(|| tags.map(|(k, v)| (k.to_owned(), v.to_owned())).collect())
}

/// Reads every element version in an osmChange document. Elements in a
/// `<delete>` block are reported as not POIs whatever their tags.
pub fn read_osc(input: impl BufRead) -> Result<Vec<ElementVersion>> {
    let mut reader = quick_xml::Reader::from_reader(input);
    let mut buf = Vec::new();
    let mut versions = Vec::new();
    let mut deleting = false;
    let mut current: Option<(ElementKey, i32)> = None;
    let mut tags: Vec<(String, String)> = Vec::new();

    let mut finish = |current: &mut Option<(ElementKey, i32)>, tags: &mut Vec<(String, String)>, deleting: bool| {
        if let Some((key, version)) = current.take() {
            let pairs = tags.iter().map(|(k, v)| (k.as_str(), v.as_str()));
            let poi_tags = if deleting { None } else { poi_tags(pairs) };
            versions.push(ElementVersion { key, version, poi_tags });
        }
        tags.clear();
    };

    loop {
        let event = reader.read_event_into(&mut buf).context("reading osmChange XML")?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                "create" | "modify" => deleting = false,
                "delete" => deleting = true,
                "node" | "way" | "relation" => {
                    current = Some(element_start(&e)?);
                    if empty {
                        finish(&mut current, &mut tags, deleting);
                    }
                }
                "tag" if current.is_some() => {
                    let (mut k, mut v) = (None, None);
                    for attr in e.attributes() {
                        let attr = attr?;
                        match attr.key.as_ref() {
                            "k" => k = Some(attr.normalized_value(XmlVersion::Explicit1_0)?.into_owned()),
                            "v" => v = Some(attr.normalized_value(XmlVersion::Explicit1_0)?.into_owned()),
                            _ => {}
                        }
                    }
                    tags.push((k.context("tag without k")?, v.context("tag without v")?));
                }
                _ => {}
            },
            Event::End(e) if matches!(e.name().as_ref(), "node" | "way" | "relation") => {
                finish(&mut current, &mut tags, deleting);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(versions)
}

fn element_start(e: &BytesStart) -> Result<(ElementKey, i32)> {
    let osm_type = match e.name().as_ref() {
        "node" => OsmType::Node,
        "way" => OsmType::Way,
        _ => OsmType::Relation,
    };
    let (mut id, mut version) = (None, None);
    for attr in e.attributes() {
        let attr = attr?;
        match attr.key.as_ref() {
            "id" => id = Some(attr.normalized_value(XmlVersion::Explicit1_0)?.parse::<i64>()?),
            "version" => version = Some(attr.normalized_value(XmlVersion::Explicit1_0)?.parse::<i32>()?),
            _ => {}
        }
    }
    let (Some(osm_id), Some(version)) = (id, version) else {
        bail!("{} without id or version", osm_type.as_str());
    };
    Ok((ElementKey { osm_type, osm_id }, version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::tags;

    #[test]
    fn reads_osc() {
        let osc = r#"<?xml version="1.0" encoding="UTF-8"?>
<osmChange version="0.6">
  <create>
    <node id="1" version="1" lat="0" lon="0"><tag k="amenity" v="cafe"/><tag k="name" v="Tom &amp; Jerry's"/></node>
    <node id="2" version="1" lat="0" lon="0"/>
  </create>
  <modify>
    <way id="3" version="4"><nd ref="1"/><tag k="building" v="yes"/></way>
  </modify>
  <delete>
    <relation id="5" version="2"><tag k="amenity" v="cafe"/><tag k="name" v="X"/></relation>
  </delete>
</osmChange>"#;
        let versions = read_osc(osc.as_bytes()).unwrap();
        let key = |osm_type, osm_id| ElementKey { osm_type, osm_id };
        assert_eq!(
            versions,
            vec![
                ElementVersion {
                    key: key(OsmType::Node, 1),
                    version: 1,
                    poi_tags: Some(tags([("amenity", "cafe"), ("name", "Tom & Jerry's")])),
                },
                ElementVersion { key: key(OsmType::Node, 2), version: 1, poi_tags: None },
                ElementVersion { key: key(OsmType::Way, 3), version: 4, poi_tags: None },
                ElementVersion { key: key(OsmType::Relation, 5), version: 2, poi_tags: None },
            ]
        );
    }
}
