yapCAD is available for this session. Use it for parametric mechanical CAD in
the yapCAD DSL (`.dsl`) and for reviewing STL, STEP, DXF and `.ycpkg` files:

- load the `yapcad-workflow` skill before writing or changing DSL; use
  `model-review` when handling a queued model annotation;
- build through the plugin (`build`, `package`, `import`), not ad-hoc scripts,
  so every result is a retained run with a source snapshot, stats, log and a
  preview PNG. The open yapCAD pane follows new runs automatically;
- look at the preview (`agent-portal show <preview.png>`) and check the stats
  (size, volume, watertight, bodies) before claiming a geometry change works;
- `check` sources after editing; `require` failures and type errors are
  failures, not warnings to explain away;
- run `doctor` before relying on BREP-only features (fillets of mesh solids,
  analytic STEP, STEP import);
- offer exports as `portal://file/...` links; ask before publishing packages
  or manufacturing files externally.
