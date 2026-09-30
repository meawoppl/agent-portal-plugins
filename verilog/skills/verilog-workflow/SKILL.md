---
name: verilog-workflow
description: Develop and test Verilog/SystemVerilog with Icarus, preserved simulation artifacts, and repository-specific testbench conventions in the Verilog Portal plugin.
---

# Verilog Development

Inspect the repository's runner, CI, source lists, include paths, vendor simulation
models, and bench assertions before changing RTL. MagicSchoolBus and fpga-tesla
use `src/verilog` and `src/verilog_test`; Widlar uses `tesla_ctl/fpga` and
`tesla_ctl/fpga_test`; gps-time-calibrator uses `fpga/rtl` and `fpga/rtl_test`.
Bench modules normally match the `_tb` filename. Discover actual paths rather
than copying a historical default from a runner.

Use the plugin's `bin/verilog doctor --cwd PROJECT`, then `test --cwd PROJECT`
or `test --cwd PROJECT --test RELATIVE_BENCH_PATH`. For nonstandard source sets,
write `.verilog-workbench.json` using the documented schema in the plugin README.
Preserve the project's existing runner as the authoritative CI comparison.

Compile with `iverilog -g2012 -gassertions`, selecting the bench top with `-s`.
Include the correct iCE40 `cells_sim.v` when needed. Keep runs separate; retain
logs, waveforms and source snapshots for failed as well as passing runs.

Legacy assertions frequently use `$display("ERROR: ...")` and `$finish`, which
can exit zero. Check both exit status and log diagnostics. For new assertions,
prefer `$fatal(1, ...)`, an explicit timescale, deterministic inputs, reset
checks, and a watchdog timeout. Check boundaries, handshake timing, and
unknown/high-impedance states relevant to the change. Waveform appearance alone
is not a functional test or proof of timing closure.

Run the affected bench first, then the relevant suite. Report existing failures
separately from regressions. Do not weaken assertions to make a run green.
Synthesis, place-and-route, and hardware programming are separate operations;
normal test/review requests do not imply flashing a connected controller.

The plugin forwards arguments after `tool --cwd PROJECT -- TOOL` unchanged to
supported installed HDL tools. Use each tool's help for evolving flags rather
than expanding a wrapper for every option. Current simulation execution is
Icarus, not a generic cocotb or Verilator test runner.
