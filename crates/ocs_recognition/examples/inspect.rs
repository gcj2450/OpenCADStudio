//! Read a real CAD fixture without launching the editor or exporting geometry.
//! Usage: cargo run -p ocs_recognition --example inspect -- INPUT [LAYER] [TOLERANCE]
use codec::{io::dwg::DwgReader, DxfReader};
use ocs_recognition::{recognize, Options};
use serde_json::json;
use std::{env, path::PathBuf, time::Instant};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("Expected a DWG or DXF path")?);
    let layer = args.next();
    let tolerance = args.next().map(|value| value.parse::<f64>()).transpose()?;
    if args.next().is_some() {
        return Err("Usage: inspect INPUT.dwg|INPUT.dxf [LAYER] [TOLERANCE]".into());
    }
    let options = Options {
        layers: layer.into_iter().collect(),
        tolerance: tolerance.unwrap_or(1.0),
        limit: 1000,
        ..Options::default()
    };
    if !options.tolerance.is_finite() || options.tolerance < 1e-9 {
        return Err("Tolerance must be finite and at least 1e-9 drawing units".into());
    }
    let started = Instant::now();
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("");
    let outcome = if extension.eq_ignore_ascii_case("dwg") {
        DwgReader::from_file(&path)?.read_with_stats()?
    } else if extension.eq_ignore_ascii_case("dxf") {
        DxfReader::from_file(&path)?.read_with_stats()?
    } else {
        return Err("Only DWG and DXF are supported".into());
    };
    let parse_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let result = recognize(&outcome.document, &options);
    let recognize_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "input": path,
            "timings": {"parse_ms": parse_ms, "recognize_ms": recognize_ms},
            "reader_stats": outcome.stats,
            "recognition": result,
        }))?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
