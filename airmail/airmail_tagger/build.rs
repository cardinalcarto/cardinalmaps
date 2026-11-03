//! Embeds libpostal's dictionaries (`dictionaries/<language>/<type>.txt`)
//! as a sorted `(path, contents)` table, so nothing is read at runtime.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read dictionaries").map(|e| e.expect("entry").path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, root, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("txt") {
            let rel = path.strip_prefix(root).expect("relative").to_string_lossy().replace('\\', "/");
            out.push((rel, path));
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("dictionaries");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();
    let mut code = String::from("/// `(path relative to dictionaries/, contents)`, sorted by path.\npub(crate) static FILES: &[(&str, &str)] = &[\n");
    for (rel, path) in &files {
        writeln!(code, "    ({rel:?}, include_str!({:?})),", path.display().to_string()).expect("write");
    }
    code.push_str("];\n");
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("dictionaries.rs"), code).expect("write");
}
