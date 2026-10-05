# Generic physical stitching controls

These small, synthetic three-pad boards retain native KiCad 10.0.6 filled copper.
They are independent of Board05's circuit, placement and reviewed routing.
`continuous.kicad_pcb` has one GNDA plane and one pad needing a via;
`split.kicad_pcb` has separated GNDA islands. Both use actual traces, plated vias,
SMD pads and filled zones, rather than matching net labels as connectivity proof.

Native reproduction at main `7368bf597eaefb2dbe171f79db3c3216843a2ae6`:

| Fixture | Native opens before geometric stitching | After stitching/refill | Vias added |
|---|---:|---:|---:|
| continuous | 1 | 0 | 1 |
| split | 2 | 1 | 2 |

The second result is geometric progress, but is not physical power completion.
These boards originated in an upstream physical-completion test. That Python
harness is not tracked here. Kicadmium uses the boards as native Rust regression
fixtures; a skipped native KiCad measurement is not acceptance evidence.

The intentionally minimal `Test` footprint library is not installed. Existing
library-availability warnings remain visible in native evidence; completion
requires no new findings, while `strict_drc=True` requires no findings at all.

For an individual user board, inspect current native command help and write a
separate output:

```sh
kicadmium kct -- stitch board.kicad_pcb --net GND --via-size 0.6 --drill 0.3 --output stitched.kicad_pcb
```

The input remains unchanged. Stitch placement is geometric; refill and verify
the explicit output with native KiCad before treating it as electrically complete.
