# Fresh Water Recognition

This OpenCADStudio plugin exposes two commands:

- `FW_RECOGNIZE [output-directory]` reads the active document through the native
  `ocs_recognition` crate and refreshes `CADAGENT_RECOGNITION`. It writes
  `structured.json` only when an output directory is supplied.
- `FW_EXPORT [output-directory]` explicitly exports the current document as
  `cad-document.json` and `drawing.dxf`, and writes the bounded native result
  to `structured.json`. Check `complete` and the per-collection page cursors;
  this result may be partial.

Recognition runs in the host process and reads its current CAD database. It
does not launch Python or export DXF during `FW_RECOGNIZE`. The reader supports
model-space lines, polylines, arc endpoints, text and nested named blocks.
It preserves source handles, distinguishes T contacts from X crossings and
excludes symbol strokes and the overlay layer from pipe candidates.

The overlay draws candidate lines and symbols for named components. Re-running
`FW_RECOGNIZE` replaces the previous marks. Arc endpoint evidence is not drawn
as a straight chord. An incomplete or paged component/step result is rejected
before altering the overlay; use `ocs_read recognize` with layer filters and
paging for those drawings. `complete` describes the work budget, not complete
domain recognition. Inspect `reviewItems`: pipe labels/diameters, equipment
ports, full routes and dynamic block visibility are not yet inferred.

`CAD_LAYER` selects the candidate centerline layer (default `GSILayerP-TG`).
`CAD_TOLERANCE` controls endpoint tolerance in drawing units (default `1`).
`CAD_OVERLAY_TEXT_HEIGHT` controls overlay text height (default `1.5`).
For `FW_EXPORT` without a directory, `CAD_AGENT_ROOT` sets the root of
`data/plugin-exports`; otherwise the current working directory is used.
`CAD_PYTHON` is no longer used by the plugin.

Build with:

```powershell
cargo build --offline --manifest-path plugins/freshwater-recognizer/Cargo.toml
```

The `cad-reader INPUT.dwg|INPUT.dxf OUTPUT_DIRECTORY` binary remains available
for compatibility document export and writes `reader-stats.json`. It is used
by the separate offline recognition CLI, not the native Agent.
