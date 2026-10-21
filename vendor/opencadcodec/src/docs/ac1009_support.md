# AC1009 / R12 DWG Support

AC1009 has native reader and writer paths, but complete R12 fidelity is still
in progress. Successfully reading a drawing is not evidence that every native
field or record arrangement survives a save.

## Issue #110 Status

In [issue #110](https://github.com/hakanaktt/acadrust/issues/110), Ottosson
[confirmed on October 13, 2026](https://github.com/hakanaktt/acadrust/issues/110#issuecomment-6139804489)
that all roughly 2,000 drawings in his private corpus read successfully,
including seven AC1009 drawings. He also explicitly stated that full AC1009
support is unfinished. The comment does not enumerate the remaining work.

The imported fixes cover common entity field widths and ordering, imported
layout and dictionary references, stored and synthesized handles, optional
font resolution, and current layer/style/linetype header-index resolution.
The private corpus is not available for independent verification.

Ottosson's additional
[`641ec9c`](https://github.com/ottosson/acadrust/commit/641ec9c35a41a88e10375d90556ffec9ef980f64)
and [`86cfbbd`](https://github.com/ottosson/acadrust/commit/86cfbbd5a0e12791535c29b5e9f1a75d8842001e)
changes are also incorporated. `DwgReader::with_r12_code_page("ANSI_1252")`
provides an explicit, validated AC1009 import encoding for header strings,
tables, and entities. Default decoding honors the file's declaration, and the
override preserves `document.header.code_page`. Saving uses that declaration
to re-encode decoded strings, including legacy escapes where needed; it does
not persist the import override. Later DWG versions are unaffected.
Fixed-width header strings that cannot fit after re-encoding return an error
instead of silently truncating text or splitting a legacy escape.

Synthesized Model/Layout1 layouts receive the plot flags, plot area, scale,
and shade resolution observed in AutoCAD imports. Printer and paper choices
remain application-profile settings. Regression coverage includes both handle
storage modes. CAD-created viewport validation is reproducible from the
BricsCAD script, but its DWG binary is not checked in.

## Fixes From This Review

The document writer now preserves native layer properties, text-style fonts
and sizing, linetype patterns, dimension-style settings, and VIEW/UCS/VPORT/VX
table rows instead of rebuilding name-only records. Current layer, text-style,
linetype, and named UCS selections use their actual table indices. VX rows are
retained directly because the DXF bridge has no VX table reader.
BLOCK table flags are applied during import and their section-relative
locators are computed during writing. Native one-byte STYLE indices decode
as unsigned values through 255; writing an index of 256 fails instead of
silently wrapping.

## Deterministic Regression Coverage

| Area | Implemented Behavior | Evidence |
| --- | --- | --- |
| JUMP framing | CRC at bytes 8-9, before padding; unsigned addresses | Independent bytes, corruption and padding cases |
| Stream traversal | Reassembles model-space POLYLINE and INSERT children across extras; removes control markers | Full DWG fixtures, three saves, AutoCAD geometry checks |
| Address validation | Absolute main-file return addresses and flagged section offsets; exact record boundaries | AutoCAD-verified return markers; cycle, orphan, owner and boundary failures |
| Import handles | Allocates in original native import order after logical reassembly | Existing independent AutoCAD identity regressions |
| SHAPE | Retains number and STYLE identity, including duplicate unnamed styles; correct native STYLE flag bits | Independent SHAPE bytes, typed edits, AutoCAD resolves BOX from ltypeshp.shx |
| Entity viewport references | Retains unsigned references on entities and ordered compound children | Zero/65535, attribute reordering, vertex addition and serde cases |
| Header retention | Preserves all 18 unmapped header spans through native saves | Independent 204/205 layouts, three saves and typed edits |
| MENU | Joins/splits the 15/45-byte fields, up to 60 encoded bytes | Multibyte/CIF boundaries, stale continuation, NUL/overflow failures |
| Native output | Valid empty-table framing and header defaults; omits synthesized system records; HANDSEED exceeds stored child handles | AutoCAD opening and AUDIT without repairs |

The positive full-drawing fixtures are exported by:

```powershell
cargo run --example r12_native_fixtures
pwsh -NoProfile -File tests/bricscad/run_r12_native.ps1 -SkipGenerate
```

The generator reuses the exact entity-stream builders from the Rust tests.
It writes seven AC1009 files, including original JUMP layouts and native saves,
to `target/ac1009-native`. The SHAPE case uses AutoCAD's installed
`ltypeshp.shx`; no proprietary font is copied into the repository. The native
harness opens copies in separate Core Console processes, checks geometry and
ownership, runs AUDIT with repairs disabled, and fails on import recovery,
audit errors, missing checks, process failure, or timeout. It never addresses
an existing CAD process.

Malformed framing, boundary values without real referenced objects, and
arbitrary opaque header-byte patterns are parser tests, not exported drawings.

## CAD-Created Corpus

BricsCAD V20.1.07 constructed seven additional native R12 DWGs, with six
independent R12 DXF exports. The DWG binaries are generated on demand by the
script described in
[`tests/fixtures/r12/cad-created`](../../../tests/fixtures/r12/cad-created/README.md);
they are not checked in.
The corpus covers named compound blocks/attributes, 2D/3D polylines, a 2-by-3
polygon mesh, a quad polyface, paper-space viewports with distinct view/snap/grid
parameters, named UCS/view definitions, drafting settings, and edited saves.

Native source files exposed and now cover:

- BLOCK names omitted from entity records and resolved through table locators.
- VX payloads between the BLOCK begin sentinel and its entity payload.
- Polygon-mesh vertices sharing bit 64 with polyface vertices, requiring
  classification by the parent type and preservation of vertex layers/handles.
- R12 MVIEW XDATA angle units, typed viewport restoration, paper-space routing,
  real VX entity/chain references, and rebasing addresses when saving.
- Numeric XDATA with standard DXF whitespace padding.
- Typed `HeaderVariables::legacy` view, drafting, clipping and user-unit
  settings, including native edits and serde retention. UNITMODE has a
  standard DXF header representation; view/snap/grid data in DXF are retained
  through the existing VPORT records.

All seven source DWGs and all seven acadrust resaves opened cleanly in AutoCAD
2027 and completed AUDIT with zero errors and no repairs during the recorded
validation run. The scripts and per-case checks are reproducible:

```powershell
pwsh -NoProfile -File tests/bricscad/create_r12_corpus.ps1
cargo run --example r12_cad_roundtrip
pwsh -NoProfile -File tests/bricscad/run_r12_native.ps1 -SkipGenerate -OutputDirectory target/ac1009-cad-created
```

## Remaining Limits

- **Block-owned extra streams.** The section walker has deterministic
  structural tests for block continuations, but AutoCAD rejects the fabricated
  block/JUMP arrangement used to explore that branch. Native validity of that
  layout is not claimed. The exported block fixture uses contiguous records;
  additional native drawings are needed to establish actual relocated-block
  layouts before treating this case as interoperable support.
  Modern BricsCAD saves, including six native Save operations with
  ISAVEPERCENT=100, still contain zero extra bytes. Generating current drawings
  therefore does not establish that historical edit-stream layout.
- **Cached header data.** Known settings now have typed native/serde state.
  Remaining undocumented bytes and derived view-coordinate caches are retained
  only through native saves. The writer normalizes 204-variable input headers
  to the 205-variable layout. DXF has no equivalent for every R12 global/cache
  field; its current view settings are stored in VPORT records.
- **Exporter differences.** BricsCAD's native R12 DWG export clears UNITMODE
  while its R12 DXF export retains it. Its global circle-zoom header slot is
  zero while the active VPORT record stores the configured value. Tests retain
  this distinction and verify explicit native header edits separately.
- **Corpus coverage.** The native-validated corpus is not exhaustive R12
  conformance. Uncommon historical edit streams and viewport linkage variants
  still need native source evidence. Ottosson's private corpus remains
  unavailable for independent reruns.

The independent
[CC0 AC1009 layout](https://github.com/michal-josef-spacek/CAD-Format-DWG-AC1009/blob/main/dwg_ac1009.ksy)
provides corroboration for native field widths and header offsets. It is a
useful reference, not an exhaustive conformance suite.

## Scope Of The Writer

R12 cannot represent modern entity types, complex linetypes, or later object
graphs. Native entity availability and native string/index limits are checked
by the R12 codecs. Support for a version does not imply lossless conversion
from a newer version.

DXF export of an R12 document currently promotes the output to R13 or later;
it is not an AC1009 DXF writer. See `tests/issue68.rs`.
