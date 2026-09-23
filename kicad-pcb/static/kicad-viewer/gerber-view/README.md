Vendored build of pastebom.com's `gerber-view` crate.

Source: https://github.com/meawoppl/pastebom.com/tree/meawoppl/gerber-view-embeddable/crates/gerber-view
Source commit used here: def0730

Build command:

```bash
wasm-pack build --release --target web --out-dir pkg
```

The workbench loads `gerber_view.js` as a browser ES module and initializes
`gerber_view_bg.wasm` explicitly from the same directory.
