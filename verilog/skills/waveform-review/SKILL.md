---
name: waveform-review
description: Investigate Verilog waveform annotations and simulation failures using run snapshots, selected signals, cursor intervals, and reproducible testbench changes.
---

# Waveform Review

Read the annotation note and attached waveform image with its run ID, source
revision, signal hierarchy, cursor interval and timescale. Time coordinates are
integer VCD ticks; multiply by the supplied seconds-per-tick for physical time.
Do not infer nanoseconds from an unlabeled integer. Preserve X and Z semantics.

Open `.verilog-runs/RUN/run.json`, `run.log`, the waveform, and the source snapshot
before attributing a transition to the current worktree. Compare current RTL
with the captured source: the user may be inspecting an older run. Trace reset,
clock edges, enables and handshakes relevant to the question. For buses, verify
width and signedness; display radix does not change the underlying logic.

Apply the requested change to authoritative RTL/test sources, never the captured
snapshot. Add or refine an assertion for the demonstrated failure when useful,
rerun the affected bench and relevant suite, and inspect the same signal/time
region. Report the new run ID, result, and evidence that addresses the note.

Handle one delivered Portal annotation at a time and return the outcome. The
Portal durable work queue owns dispatch of the next item; do not implement a
second queue or send yourself prompts. Completion is not user acceptance. If
the capture or intent is ambiguous, identify the missing detail instead of
guessing at a circuit change.

The pane preserves signal selections, per-signal radix, cursors and time range
per project/test in browser storage. It supports VCD up to 64 MiB and limits
transitions per visible signal range. Download a larger waveform for GTKWave or
narrow `$dumpvars` scope; do not silently discard edges. GTKWave save files and
translation filters remain useful external artifacts but are not imported by
this version of the pane.
