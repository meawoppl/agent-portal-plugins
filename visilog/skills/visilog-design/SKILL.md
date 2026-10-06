---
name: visilog-design
description: Explore Verilog/SystemVerilog structure interactively with the Visilog Portal plugin: module hierarchy diagrams, live values, source inspection, pinned waveforms, stepping, breakpoints and JSON design graphs.
---

# Visilog design exploration

Use this plugin to understand how a design is built and how values move through
it. It is a functional simulator and viewer, not a verification reference:
keep the project's own runner (or the Verilog plugin's Icarus runs) as the
authority on pass/fail, and do not claim timing verification from Visilog.

Start with `bin/visilog doctor --cwd PROJECT`. It reports whether the managed
binary is installed and lists the benches it discovered: `_tb.v`/`_tb.sv`
files with sibling RTL, or the explicit tests in `.verilog-workbench.json`
(same schema as the Verilog plugin, documented in its README). Install the
pinned toolchain with `bin/visilog setup-visilog`; it builds into the plugin's
`.tools/` with Cargo and does not touch the system.

For structure questions, prefer `bin/visilog graph --cwd PROJECT --test ID`.
It prints the elaborated hierarchy as JSON (instances, ports, signals,
connections, processes) without running anything, which is cheaper and more
precise than reading screenshots. Use the pane when the user wants to look:
select a bench, Load design, then step edges, pin signals and set expression
breakpoints in the embedded viewer.

Load design always re-elaborates the current sources and resets execution. It
is not a snapshot: edits made after loading are not reflected until the next
load, and there is no resumable checkpoint. Each load writes a `viewer.log`
and any `$dumpfile` output under `.visilog-runs/`; add that directory to the
project's ignore file.

`bin/visilog tool --cwd PROJECT -- run|graph|serve|compare ...` forwards
arguments unchanged to the pinned binary. Consult `tool -- --version` and the
upstream usage text rather than guessing flags. Unsupported constructs exit
with status 3; report that honestly instead of working around it silently.
