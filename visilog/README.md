# Visilog

Agent Portal plugin for interactive Verilog design exploration. It embeds the
native [Visilog](https://github.com/meawoppl/visilog) hierarchy viewer: nested
module diagrams with live values, source inspection, pinned waveforms, edge
stepping and expression breakpoints. Batch simulation and retained waveform
review stay in the sibling `verilog` plugin; this one is about looking at how a
design is built and how it behaves step by step.

## Install and run

Requires Python 3.11+, uv and Node/npm. Building the pinned Visilog binary needs
Cargo and a native toolchain; it is installed into the plugin's `.tools/`
directory and never into the system. `VISILOG_BIN` selects a development binary.

```sh
agent-portal plugin install /path/to/agent-portal-plugins/visilog
agent-portal plugin setup visilog          # npm ci for the local ELK layout bundle
bin/visilog setup-visilog                  # cargo install of the pinned revision
agent-portal plugin doctor visilog
agent-portal plugin open visilog
```

From this directory, standalone operation:

```sh
bin/visilog setup
bin/visilog setup-visilog
bin/visilog doctor --cwd /path/to/project
bin/visilog graph --cwd /path/to/project --test rtl_test/counter_tb.v
bin/visilog serve --cwd /path/to/project --port 49130
bin/visilog tool --cwd /path/to/project -- run -s top_tb rtl/*.v
```

The pane is at http://localhost:49130. Pick a bench and press Load design; the
plugin starts `visilog serve` for that bench, waits for it to elaborate, and
proxies the upstream viewer under `/design/` on the same origin. Only one
design is served at a time, and the viewer process is stopped on reload or
shutdown. ELK layout is served from the locally installed `elkjs`; nothing is
fetched from a CDN.

## Project conventions

Bench discovery matches the Verilog plugin so the two agree on what a test is:
`_tb.v`/`_tb.sv` files with same-directory RTL (or the sibling source
directory for `*_test` folders), or explicit entries in
`.verilog-workbench.json` with `id`, `top`, `sources`, `includes`, `defines`,
`plusargs` and `timeout`. Vendor cell models resolve from `VERILOG_CELLS_SIM`,
project-local Yosys `cells_sim.v` copies, then the system Yosys share paths.

Each load writes a `design-*` directory under `.visilog-runs/` with the exact
command, `viewer.log`, and any `$dumpfile` or `$fopen` output. Add
`.visilog-runs/` to the design repository's ignore file. Loads re-elaborate
the current sources; they are not immutable snapshots and there is no
resumable checkpoint.

## Limits

Visilog performs functional simulation. `specify` timing and switch-level
delays are not simulated, so it is not a timing sign-off tool. Constructs it
does not support exit with status 3 and the error is shown in the pane.
Annotation capture of the diagram is not yet wired to the Portal edit stack.
The pinned revision is recorded in `visilog_bridge.py`; bumping it requires
re-checking the small asset adapter that rewrites viewer URLs.

## Testing

```sh
uv run --locked python -m unittest discover -s tests -v
node --check static/app.js
bin/visilog serve --cwd examples/counter --port 49130 &
uv run python tests/browser_smoke.py
```

The lifecycle tests are skipped when no Visilog binary is installed.
