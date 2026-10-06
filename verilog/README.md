# Verilog Workbench

Agent Portal plugin for Icarus simulation and waveform review. Run a bench,
inspect its retained VCD and log, and queue a waveform annotation to the session.
Interactive hierarchy exploration lives in the sibling [`visilog`](../visilog/)
plugin, which shares this plugin's bench discovery and config schema.

## Install and run

Requires Python 3.11+, uv, Node/npm, and Icarus Verilog (`iverilog`, `vvp`) on PATH.
Install Icarus and Yosys through your platform package manager; setup does not
silently modify the system. Python and icon dependencies are locked locally.

```sh
agent-portal plugin install /path/to/agent-portal-plugins/verilog
agent-portal plugin setup verilog
agent-portal plugin doctor verilog
agent-portal plugin open verilog
```

From this directory, standalone operation:

```sh
bin/verilog setup
bin/verilog doctor --cwd /path/to/project
bin/verilog test --cwd /path/to/project --test src/verilog_test/UartTx_tb.v
bin/verilog serve --cwd /path/to/project --port 49120
bin/verilog tool --cwd /path/to/project -- verilator --lint-only -Wall rtl/top.sv
```

The standalone pane is at http://localhost:49120. Annotation submission requires
the Portal proxy's `/__portal/edit-stack` endpoint; standalone errors preserve
the note. The pane submits screenshots, run identity, source digest, selected
signals, VCD tick interval and timescale to the same durable queue as KiCad.
Voice notes use browser speech recognition when supported.

## Project conventions

Autodiscovery finds `_tb.v`/`_tb.sv` files and same-directory RTL, or the sibling
source directory for `*_test` folders. It selects the filename stem as the top,
compiles all sibling `.v`/`.sv` RTL with `-g2012 -gassertions`, then runs `vvp`.
It excludes other benches. Each run has its own working directory so common
`output.vcd` filenames do not collide. Zero-exit logs containing error/fatal/fail
diagnostics fail the run, matching the older runners' assertion convention.

For custom tops, nested sources, include directories, defines or plusargs, use
an explicit `.verilog-workbench.json`. Paths are relative to the project root;
source globs are deliberately not implicit. Unknown fields, duplicate IDs,
missing inputs, invalid timeouts and paths escaping the root are rejected.

```json
{
  "version": 1,
  "tests": [{
    "id": "uart",
    "top": "uart_tb",
    "sources": ["rtl/uart.sv", "tests/uart_tb.sv"],
    "includes": ["rtl"],
    "defines": ["SIMULATION=1"],
    "plusargs": ["+seed=42"],
    "timeout": 60
  }]
}
```

Vendor models resolve from `VERILOG_CELLS_SIM`, project-local
`fpga/third_party/yosys/cells_sim.v` or `third_party/yosys/cells_sim.v`, then
`/usr/share/yosys/ice40/cells_sim.v` and `/usr/local/share/yosys/ice40/cells_sim.v`.
The selected model is recorded in the compiled source snapshot.

Runs live in `.verilog-runs/` (add it to the design repo's ignore file). They
contain `run.json`, `run.log`, snapshotted source/header files, `sim.vvp`, and
waveforms. Source digest includes bench configuration, source files and headers.
External runtime data such as `$readmemh` files are not copied automatically;
projects needing them require a future explicit staging adapter. Do not claim
such tests are supported by autodiscovery. Runs are retained until manually
removed; the UI lists the newest 100.

## Waveform controls

Select signals in the hierarchy, click a signal row to select its radix, click
the waveform for cursor A, and use the pointer-mode menu for cursor B or pan.
Two markers measure an interval. Wheel pans; Ctrl/Cmd-wheel zooms at the pointer.
Touch uses one finger for the selected mode and two fingers for pan/zoom.
Fit restores the full time range; previous/next move to a transition in the
visible window of the selected signal. Browser storage preserves views per
project/test. File changes invalidate the test list over WebSocket without
automatically running simulations or replacing historical results.

This release supports digital and numeric VCD traces via `vcdvcd`; real values
are labeled numerically, not plotted as analog curves. It caps VCD input at
64 MiB, selected signals at 64, and events per signal/window at 100,000. It
reports limits instead of dropping transitions. FST, `.gtkw` import, enum
translation, synthesis reports and arbitrary CI runners are not yet pane
features. Existing GTKWave sessions remain available outside the plugin.

## Testing

```sh
uv run --locked python -m unittest discover -s tests -v
bin/verilog test --cwd examples/counter
node --check static/app.js
```

## Sources and conventions inspected

- [GTKWave](https://gtkwave.github.io/gtkwave/ui/overview.html): hierarchy,
  signal values beside traces, cursor measurement, zoom and saved view state.
- [MagicSchoolBus](https://github.com/meawoppl/MagicSchoolBus): `endiatx.py`, CI,
  `src/verilog_test`, SystemVerilog assertions and `ERROR` diagnostics.
- [Widlar](https://github.com/coup-de-foudre/widlar): `tool.py`,
  `tesla_ctl/fpga_test`, pulse/MIDI/SPI benches and persistent GTKWave views.
- [fpga-tesla](https://github.com/coup-de-foudre/fpga-tesla): `dev_tool.py`,
  `src/verilog_test`, the early iCE40 controller workflow.
- Local `gps-time-calibrator`: `fpga/rtl_test`, vendored Yosys simulation model
  and inherited Icarus runner. Local `affogato`: Verilator lint and FPGA RTL.
- [vcdvcd](https://github.com/cirosantilli/vcdvcd): waveform parser, preserving
  signal aliases and event values rather than a custom VCD grammar.

The inspected repositories include legacy configurations and external forks;
the plugin does not assume every repository uses an identical test runner.
