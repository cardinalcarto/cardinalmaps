//! Embeds every file under `resources/` (dictionaries, postcode metadata) into
//! the crate as a sorted `(relative path, contents)` table, so the parser
//! needs no files at runtime.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("read resources dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, root, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("txt" | "json" | "tsv")
        ) {
            let rel = path
                .strip_prefix(root)
                .expect("relative path")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
}

fn main() {
    let root =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("resources");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();

    let mut code = String::from(
        "/// Embedded resource files: `(path relative to resources/, contents)`, sorted by path.\n",
    );
    code.push_str("pub(crate) static FILES: &[(&str, &str)] = &[\n");
    for (rel, path) in &files {
        writeln!(
            code,
            "    ({rel:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .expect("write");
    }
    code.push_str("];\n");
    let out =
        PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("embedded_resources.rs");
    std::fs::write(out, code).expect("write generated resources");
}
