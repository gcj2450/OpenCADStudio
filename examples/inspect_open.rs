//! Profile the native editor loader without creating a window or changing the drawing.
//! Usage: cargo run --example inspect_open -- INPUT.dwg [RUNS]
use std::{env, path::PathBuf, sync::Arc, time::Instant};
use OpenCADStudio::io::{open_path_with_phase, OpenProgressState};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("Expected a DWG or DXF path")?);
    let runs = args
        .next()
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(1);
    if args.next().is_some() || !(1..=20).contains(&runs) {
        return Err("Usage: inspect_open INPUT.dwg|INPUT.dxf [RUNS:1..20]".into());
    }
    let mut results = Vec::new();
    for run in 1..=runs {
        let started = Instant::now();
        let (_, _, mut document, mut caches) = iced::futures::executor::block_on(open_path_with_phase(
            path.clone(),
            Arc::new(OpenProgressState::new(0)),
            [33.0 / 255.0, 40.0 / 255.0, 48.0 / 255.0, 1.0],
        ))
        .map_err(|error| error.to_string())?;
        let loader_ms = started.elapsed().as_secs_f64() * 1000.0;
        if caches.prepared_geometry.is_none() {
            let geometry_started = Instant::now();
            let (prepared_doc, geometry) = OpenCADStudio::scene::prepare_open_geometry(
                document,
                &caches,
                [33.0 / 255.0, 40.0 / 255.0, 48.0 / 255.0, 1.0],
            );
            document = prepared_doc;
            caches.timings.finalize_ms = geometry_started.elapsed().as_millis() as u32;
            caches.prepared_geometry = Some(geometry);
        }
        results.push(serde_json::json!({
            "run": run,
            "total_ms": started.elapsed().as_secs_f64() * 1000.0,
            "loader_return_ms": loader_ms,
            "entities": document.entity_count(),
            "phases_ms": {
                "parse": caches.timings.parse_ms,
                "purge": caches.timings.purge_ms,
                "references": caches.timings.xref_ms,
                "caches": caches.timings.caches_ms,
                "geometry": caches.timings.finalize_ms,
            },
            "reader_stats": caches.read_stats,
            "geometry_prepared": caches.prepared_geometry.is_some(),
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "input": path,
            "includes": "native loader, fixups, references, caches, prepared model geometry",
            "excludes": "UI installation, GPU upload, first presented frame",
            "runs": results,
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
