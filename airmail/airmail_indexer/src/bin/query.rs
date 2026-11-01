//! Interactive search against an Airmail index, with the categorical model's
//! reading of each query. With only `--categories`, shows the model's output
//! without searching.

use airmail::{
    categories::{self, CategoryModels, DEFAULT_INCLUDE_THRESHOLD, QueryCategories},
    index::AirmailIndex,
    poi::AirmailPoi,
};
use clap::Parser;
use geo::Coord;
use rustyline::{DefaultEditor, error::ReadlineError};

#[derive(Debug, Parser)]
struct Args {
    /// Index to search. Without it, only the categorical model runs.
    #[clap(long, short)]
    index: Option<String>,
    /// Restrict results to `min_lng,min_lat,max_lng,max_lat`
    #[clap(long, short)]
    bbox: Option<String>,
    /// Categorical search model to understand queries with: a model file or a
    /// directory of them
    #[clap(long)]
    categories: Option<std::path::PathBuf>,
    /// Query language, such as `de` or `de-CH`; picks the model. Optional when
    /// only one model is loaded.
    #[clap(long)]
    lang: Option<String>,
    /// Minimum model confidence to also return places matching only by category
    #[clap(long, default_value_t = DEFAULT_INCLUDE_THRESHOLD)]
    category_include_threshold: f32,
}

fn parse_bbox(s: &str) -> geo::Rect {
    let parts: Vec<f64> = s
        .split(',')
        .map(|p| p.trim().parse().expect("bbox values must be numbers"))
        .collect();
    let [min_lng, min_lat, max_lng, max_lat] = parts[..] else {
        panic!("Invalid bbox format. Need: `lng1,lat1,lng2,lat2`");
    };
    geo::Rect::new(
        Coord {
            y: min_lat,
            x: min_lng,
        },
        Coord {
            y: max_lat,
            x: max_lng,
        },
    )
}

fn format_scored(values: &[(String, f32)]) -> String {
    if values.is_empty() {
        return "-".to_string();
    }
    values
        .iter()
        .take(5)
        .map(|(v, p)| format!("{v} {p:.2}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_understood(understood: &QueryCategories, threshold: f32) {
    println!("  categories: {}", format_scored(&understood.categories));
    println!(
        "  food {:.2}   cuisines: {}   diets: {}",
        understood.food,
        format_scored(&understood.cuisines),
        format_scored(&understood.diets)
    );
    println!(
        "  mode: {:?} (primary confidence {:.2}, threshold {threshold:.2})",
        understood.mode(threshold),
        understood.primary_confidence()
    );
}

/// One line per result: score, name (or address), category tags, cuisine and diets.
fn format_poi(poi: &AirmailPoi, score: f32) -> String {
    let tag = |key: &str| {
        poi.tags
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let name = tag("name").map_or_else(
        || {
            format!(
                "{} {}",
                tag("addr:housenumber").unwrap_or_default(),
                tag("addr:street").unwrap_or("(unnamed)")
            )
        },
        str::to_string,
    );
    let mut details: Vec<String> = poi
        .tags
        .iter()
        .filter(|(k, v)| {
            categories::is_category_tag(k, v) || k == "cuisine" || k.starts_with("diet:")
        })
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    details.sort();
    format!(
        "{score:7.2}  {name}  [{}]  ({:.4}, {:.4})",
        details.join(" "),
        poi.lat,
        poi.lng
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.index.is_none() && args.categories.is_none() {
        return Err("pass --index, --categories, or both".into());
    }
    let index = args.index.as_deref().map(AirmailIndex::new).transpose()?;
    let models = args
        .categories
        .as_deref()
        .map(CategoryModels::open)
        .transpose()?
        .unwrap_or_default();
    let model = match (&args.lang, models.languages().as_slice()) {
        (Some(lang), _) => Some(
            models
                .get(lang)
                .ok_or_else(|| format!("no model for {lang}; have {:?}", models.languages()))?,
        ),
        (None, [only]) => models.get(only),
        (None, []) => None,
        (None, many) => return Err(format!("pass --lang to pick one of {many:?}").into()),
    };
    if let Some(model) = model {
        println!("categorical model: {}", model.language());
    }
    let bbox = args.bbox.as_deref().map(parse_bbox);
    let mut rl = DefaultEditor::new()?;

    loop {
        let line = match rl.readline("query: ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        if line.trim().is_empty() {
            continue;
        }
        rl.add_history_entry(line.as_str())?;
        let start = std::time::Instant::now();

        // The model gets the raw query, as in the service.
        let understood = model.map(|m| m.understand(&line));
        if let Some(understood) = &understood {
            print_understood(understood, args.category_include_threshold);
            println!("  understood in {:?}", start.elapsed());
        }
        let Some(index) = &index else { continue };

        // The index holds ASCII-transliterated text, as the service searches it.
        let query = deunicode::deunicode(line.trim()).to_lowercase();
        let start = std::time::Instant::now();
        let mut results = index
            .search(
                &query,
                true,
                None,
                understood
                    .as_ref()
                    .map(|c| (c, c.mode(args.category_include_threshold))),
                bbox,
                &[],
            )
            .await?;
        results.sort_by(|(_, a), (_, b)| b.total_cmp(a));
        for (poi, score) in results.iter().take(10) {
            println!("{}", format_poi(poi, *score));
        }
        println!("{} results in {:?}\n", results.len(), start.elapsed());
    }
}
