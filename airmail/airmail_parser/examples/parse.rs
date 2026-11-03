//! Prints the parse of a query: solutions and per-token weights.
//!
//! `cargo run --release -p airmail_parser --example parse -- 123 main st springfield il`

use std::fmt::Write as _;

fn main() {
    let query = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let parse = airmail_parser::parse(&query);
    println!("{query:?}");
    for s in parse.solutions() {
        let mut line = format!("  ({:.3})", s.score);
        for span in &s.spans {
            write!(
                line,
                " {}={:?}@{:.2}",
                span.kind.label(),
                span.text,
                span.confidence
            )
            .expect("write");
        }
        println!("{line}");
    }
    for tw in parse.token_weights() {
        let mut line = format!("  {:<14}", tw.token.text);
        for (kind, w) in &tw.weights.kinds {
            write!(line, " {}={w:.2}", kind.label()).expect("write");
        }
        write!(line, " unclassified={:.2}", tw.weights.unclassified).expect("write");
        println!("{line}");
    }
    if std::env::var_os("FEATURES").is_some() {
        for f in parse.features() {
            println!(
                "  feature {:?} {} {:.2}",
                f.text,
                f.kind.label(),
                f.confidence
            );
        }
    }
}
