//! Interactive search against an Airmail index, with the categorical model's
//! reading of each query. With only `--categories`, shows the model's output
//! without searching.

use airmail::{
    categories::{self, CategoryModels, QueryCategories},
    index::{AirmailIndex, SearchHit, SearchRequest},
    poi::{Kind, osm_display_name},
};
use clap::Parser;
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
    /// Match the last word as a prefix
    #[clap(long)]
    prefix: bool,
    /// Tolerate typos
    #[clap(long)]
    typos: bool,
}

fn parse_bbox(s: &str) -> [f64; 4] {
    let parts: Vec<f64> = s
        .split(',')
        .map(|p| p.trim().parse().expect("bbox values must be numbers"))
        .collect();
    let [min_lng, min_lat, max_lng, max_lat] = parts[..] else {
        panic!("Invalid bbox format. Need: `lng1,lat1,lng2,lat2`");
    };
    [min_lng, min_lat, max_lng, max_lat]
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

fn print_understood(understood: &QueryCategories) {
    println!("  categories: {}", format_scored(&understood.categories));
    println!(
        "  food {:.2}   cuisines: {}   diets: {}",
        understood.food,
        format_scored(&understood.cuisines),
        format_scored(&understood.diets)
    );
}

/// One line per result: score, kind, name (or address), category tags,
/// cuisine and diets.
fn format_hit(hit: &SearchHit) -> String {
    let name = match (&hit.wof, osm_display_name(&hit.tags, None)) {
        (Some(wof), _) => format!("{} ({})", wof.default_name, wof.placetype),
        (None, Some(name)) => name.to_string(),
        (None, None) => format!(
            "{} {}",
            hit.tag("addr:housenumber").unwrap_or_default(),
            hit.tag("addr:street").unwrap_or("(unnamed)")
        ),
    };
    let mut details: Vec<String> = hit
        .tags
        .iter()
        .filter(|(k, v)| categories::is_category_tag(k, v) || k == "cuisine" || k.starts_with("diet:"))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    details.sort();
    format!(
        "{:7.2}  {:<7}  {name}  [{}]  ({:.4}, {:.4})  importance {:.2}",
        hit.score,
        hit.kind.as_str(),
        details.join(" "),
        hit.center.y(),
        hit.center.x(),
        hit.importance,
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
            print_understood(understood);
            println!("  understood in {:?}", start.elapsed());
        }
        let Some(index) = &index else { continue };

        let start = std::time::Instant::now();
        let request = SearchRequest {
            text: line.trim(),
            prefix: args.prefix,
            typos: args.typos,
            model,
            bbox,
            kinds: Kind::ALL.to_vec(),
            limit: 10,
            ..Default::default()
        };
        let results = index.search(&request)?;
        for hit in &results.hits {
            println!("{}", format_hit(hit));
        }
        println!("{} results in {:?}\n", results.hits.len(), start.elapsed());
    }
}
