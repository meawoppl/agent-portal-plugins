# Wave C parity audit

The native Rust Wave C commands were differentially checked against
`rjwalters/kicad-tools` at upstream commit `37665de5`.

The five historical board fixtures exercise unrouted, routed, plane-heavy,
high-pin-count, differential-pair, curved-copper, and thermal-source designs.
Canonical JSON output matched exactly for:

- `pcb summary|footprints|nets|padmap|traces|zones`
- `analyze trace-lengths --all|complexity|congestion|signal-integrity|thermal`
- `net-status`
- `estimate cost`

The recursive help audit found no missing Wave C option flags. Render and
screenshot behavior remains delegated to the discovered external `kicad-cli`,
as required by kicadmium's no-bundled-KiCad boundary.

Run the crate regression suite with `cargo test -p kct -j4 --lib`.
