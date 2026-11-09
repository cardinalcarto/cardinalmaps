//! Compiles GTFS feeds into a timetable file for `valinor-transit`.

use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use clap::Parser;
use valinor_gtfs::{FeedId, FeedInput, ZipSource, compile};

/// Compiles GTFS zip archives into one timetable file.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// A feed as `ID=PATH`, where `ID` is the namespace for its identifiers.
    /// Repeat for several feeds, which must share a time zone.
    #[arg(long = "feed", value_name = "ID=PATH", required = true)]
    feeds: Vec<String>,
    /// Where to write the compiled file.
    #[arg(long, value_name = "PATH")]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let mut inputs = Vec::with_capacity(args.feeds.len());
    for feed in &args.feeds {
        let (id, path) = feed
            .split_once('=')
            .ok_or_else(|| format!("feed {feed:?} is not in ID=PATH form"))?;
        let id = FeedId::try_new(id.to_owned())?;
        let source = ZipSource::from_path(&PathBuf::from(path))?;
        inputs.push(FeedInput {
            id,
            source: Box::new(source),
        });
    }
    let output = compile(inputs)?;

    // Writing to a temporary name keeps a reader from mapping a half-written file.
    let partial = args.output.with_extension("part");
    {
        let mut file = BufWriter::new(File::create(&partial)?);
        file.write_all(&output.bytes)?;
        file.flush()?;
    }
    fs::rename(&partial, &args.output)?;

    for report in &output.reports {
        println!("{report:#?}");
    }
    println!(
        "wrote {} bytes to {}",
        output.bytes.len(),
        args.output.display()
    );
    Ok(())
}
