use ocs_plugin_api::host::codec::io::{dxf::DxfReader, dwg::DwgReader};
use ocs_freshwater_recognizer::export_document;
use std::{env, path::Path};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "--version-info" {
        println!("{}", ocs_plugin_api::get_embedded_version_info_json());
        return Ok(());
    }
    if args.len() != 3 { return Err("Usage: cad-reader INPUT.dwg|INPUT.dxf OUTPUT_DIRECTORY".into()); }
    let path = Path::new(&args[1]);
    let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    let outcome = match extension.as_str() {
        "dwg" => DwgReader::from_file(path)?.read_with_stats()?,
        "dxf" => DxfReader::from_file(path)?.read_with_stats()?,
        _ => return Err("Only DWG and DXF are supported".into()),
    };
    export_document(&outcome.document, Path::new(&args[2]))?;
    std::fs::write(Path::new(&args[2]).join("reader-stats.json"), serde_json::to_vec_pretty(&outcome.stats)?)?;
    println!("CAD database exported to {}", args[2]);
    Ok(())
}
fn main() {
    if let Err(error) = run() { eprintln!("{error}"); std::process::exit(1); }
}
