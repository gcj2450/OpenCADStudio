# Native CAD Validation

The native loader and recognition paths can be profiled without starting the
desktop window or creating an intermediate DXF/Python process.

## Loader profile

Run the full OpenCADStudio open pipeline with a DWG or DXF:

```text
cargo run --example inspect_open -- INPUT.dwg [RUNS]
```

The JSON report includes total time, entity count, parser statistics, and
separate `parse`, `purge`, `references`, `caches`, and `geometry` timings. It
includes fixups, xref resolution, derived caches, and prepared model geometry;
it excludes UI installation, GPU upload, and the first presented frame.

## Recognition profile

Run the native structural recognizer directly:

```text
cargo run -p ocs_recognition --example inspect -- INPUT.dwg [LAYER] [TOLERANCE]
```

This reports bounded visits, candidate segments, topology, recognized
components, annotation text, and review items. `complete: false` means the
recognition budget or an unsupported structure requires another pass or review;
it does not mean that the drawing failed to parse.

## Recorded fixture

For `data/uploads/a33d74ca-fd5d-4302-86ec-25b734c77439/M-28-K4660431-RA-Fresh water production-20260306_Block.dwg`, the full native loader
measured about 343,437 ms on the development machine: parse 906 ms, purge
31 ms, derived caches 397 ms, and prepared geometry 342,096 ms. It produced
94,759 entities with no parser recovery records. The direct recognizer measured
about 408 ms and reached its 2,000-segment budget (`complete: false`). A small
`freshwater-demo.dxf` completed in about 683 ms, including 670 ms of geometry
preparation. The large-DWG result identifies geometry preparation as the next
performance bottleneck; it is not a first-presented-frame measurement.
