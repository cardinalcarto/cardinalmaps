//! Serves the device's offline index over HTTP with the server's `/v1/search` shape, so
//! the geocoder server's evaluation scripts (`airmail/eval`) and the offline/server
//! comparison (`bench_quality.py`, next to this file) can query both the same way.
//! A benchmark tool only: one request at a time, no keep-alive.
//!
//!   offline_serve --index <dir> [--tagger tagger.model] [--categories eng.amcats]
//!       [--bind 127.0.0.1:3920]
//!
//! `/v1/place?id=` looks a result ID up, as the server's does. `/v1/search` takes `q`,
//! `lang`, `prefix`, `typos`, `focus`, `viewport`, `bbox` and `limit` (default 10, as the
//! server). The device has no `within`, `kind`, `tags` or `location`: they're
//! ignored, and listed in the response's `query.ignored`. `query.search_ms` is the time
//! the search itself took, without HTTP.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Instant;

use geocoder_flutter::search::{DeviceIndex, Query};
use serde_json::{Value, json};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut take = |flag: &str| {
        let i = args.iter().position(|a| a == flag)?;
        args.remove(i);
        Some(args.remove(i))
    };
    let dir = PathBuf::from(take("--index").expect("--index"));
    let tagger = take("--tagger").map(PathBuf::from);
    let categories = take("--categories").map(PathBuf::from);
    let bind = take("--bind").unwrap_or_else(|| "127.0.0.1:3920".to_string());
    let index = DeviceIndex::open(&dir, tagger.as_deref(), categories.as_deref()).unwrap();
    eprintln!(
        "{} documents (tagger {}, categories {:?}); listening on {bind}",
        index.documents().unwrap(),
        tagger.is_some(),
        index.category_language()
    );
    let listener = TcpListener::bind(&bind).unwrap();
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if let Err(e) = handle(&index, stream) {
            eprintln!("request failed: {e}");
        }
    }
}

fn handle(index: &DeviceIndex, mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    // Skip the headers.
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" || header == "\n" {
            break;
        }
    }
    let target = line.split_whitespace().nth(1).unwrap_or("/");
    let (path, raw) = target.split_once('?').unwrap_or((target, ""));
    let (status, body) = match path {
        "/health" => ("200 OK", json!({ "ok": true })),
        "/v1/search" => match search(index, raw) {
            Ok(body) => ("200 OK", body),
            Err(e) => ("400 Bad Request", json!({ "error": e })),
        },
        "/v1/place" => {
            let params = params(raw);
            let id = params
                .iter()
                .find(|(k, _)| k == "id")
                .map(|(_, v)| v.as_str());
            let lang = params
                .iter()
                .find(|(k, _)| k == "lang")
                .map(|(_, v)| v.as_str());
            match id.map(|id| index.place(id, lang)) {
                Some(Ok(Some(place))) => ("200 OK", json!({ "result": place })),
                Some(Ok(None)) => ("404 Not Found", json!({ "error": "no such place" })),
                Some(Err(e)) => (
                    "500 Internal Server Error",
                    json!({ "error": e.to_string() }),
                ),
                None => ("400 Bad Request", json!({ "error": "id is required" })),
            }
        }
        _ => ("404 Not Found", json!({ "error": "not found" })),
    };
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn search(index: &DeviceIndex, raw: &str) -> Result<Value, String> {
    let mut query = Query {
        limit: 10,
        ..Query::default()
    };
    let mut ignored = Vec::new();
    for (key, value) in params(raw) {
        match key.as_str() {
            "q" => query.text = value,
            "lang" => query.lang = Some(value),
            "prefix" => query.prefix = value == "true",
            "typos" => query.typos = value == "true",
            "limit" => query.limit = value.parse().map_err(|_| "limit")?,
            "focus" => query.focus = Some(numbers(&value)?),
            "viewport" => query.viewport = Some(numbers(&value)?),
            "bbox" => query.bbox = Some(numbers(&value)?),
            _ => ignored.push(key),
        }
    }
    if query.text.trim().is_empty() {
        return Err("q is required".into());
    }
    let start = Instant::now();
    let results = index.search(&query).map_err(|e| e.to_string())?;
    let search_ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok(json!({
        "query": {
            "text": query.text,
            "lang": query.lang,
            "prefix": query.prefix,
            "typos": query.typos,
            "focus": query.focus,
            "viewport": query.viewport,
            "bbox": query.bbox,
            "limit": query.limit,
            "ignored": ignored,
            "search_ms": search_ms,
        },
        "results": results,
    }))
}

fn params(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn numbers<const N: usize>(text: &str) -> Result<[f64; N], String> {
    let values: Vec<f64> = text
        .split(',')
        .map(|v| {
            v.trim()
                .parse::<f64>()
                .map_err(|_| format!("not a number: {v}"))
        })
        .collect::<Result<_, _>>()?;
    values.try_into().map_err(|_| format!("need {N} numbers"))
}

/// Decodes `application/x-www-form-urlencoded` text.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                    .ok_or(())
                {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
