# Unlinked

Agent Portal plugin for reviewing Simulink models without MATLAB, built on
[Unlinked](https://github.com/CosmicFrontierLabs/unlinked): import `.slx` and
legacy `.mdl` files, render the root diagram and nested subsystems as SVG,
inspect block inventories and solver settings, run the supported simulation
subset and plot the traces, and transpile MATLAB scripts to Rust.

Unlinked is an independent, partial implementation. Rendering does not mean a
model can be simulated, and simulation results are not a MathWorks equivalence
claim. Graphical Stateflow execution, toolbox library behaviour, MATLAB
callbacks and general masked or conditional subsystems are not implemented;
unsupported blocks fail with a diagnostic that names them.

## Install and run

Requires Python 3.11+ and uv. Building the pinned CLI needs Cargo; it installs
into the plugin's `.tools/` directory and never into the system. `UNLINKED_BIN`
selects a development binary.

```sh
agent-portal plugin install /path/to/agent-portal-plugins/unlinked
agent-portal plugin setup unlinked          # cargo install of the pinned revision
agent-portal plugin doctor unlinked
agent-portal plugin open unlinked
```

From this directory, standalone operation:

```sh
bin/unlinked setup
bin/unlinked doctor --cwd /path/to/project
bin/unlinked serve --cwd /path/to/project --port 49140
bin/unlinked tool --cwd /path/to/project -- info controller.slx
bin/unlinked tool --cwd /path/to/project -- render controller.slx --system Plant -o plant.svg
bin/unlinked tool --cwd /path/to/project -- sim controller.slx --stop 10 --step 0.01 --solver rk4 -o trace.csv
bin/unlinked tool --cwd /path/to/project -- transpile params.m -o params-rust
```

The pane is at http://localhost:49140. Pick a model: the Diagram view renders
it and lists its subsystems when there are any; Model info holds the full
`info` JSON. The Simulate view is prefilled from the imported solver settings
(`ode1`, `ode4` and `ode45` map to Euler, RK4 and RK45; anything else defaults
to RK4) and accepts workspace values as `NAME=EXPR` lines. Runs plot on a
canvas with per-signal toggles; signals are labelled from the diagram's block
names where the SID is visible in the rendered system.

## Conventions and limits

Models are discovered anywhere under the project except ignored directories.
Each pane run writes `run.json`, `run.log` and `trace.json` under
`.unlinked-runs/<id>/`; add that directory to the design repository's ignore
file. The pane keeps the newest 50 runs. One simulation runs at a time and is
killed after 120 s. Explicit solver settings always override the imported
configuration. Scripts next to a model are never executed: read the parameters
file and pass values explicitly.

The pinned upstream revision is recorded in `server.py`. Bump it by changing
`REVISION`, rerunning `setup`, and re-running the tests below.

## Testing

```sh
uv run --locked python -m unittest discover -s tests -v
node --check static/app.js
bin/unlinked serve --cwd examples --port 49140 &
uv run python tests/browser_smoke.py
```

The CLI-backed tests are skipped when no `unlinked` binary is installed. The
examples are copied from upstream: two control loops and a nested fixture.
