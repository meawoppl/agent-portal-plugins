# kicadmium

> KiCad's toxic uncle everyone warned you about.

He shows up uninvited, reads your whole board, tells you your vias are ugly,
and he's right. kicadmium is a single Rust binary that wraps a real KiCad
install with a browser workbench (schematic, PCB, Gerbers, 3D/STEP, BOM,
libraries, checks), fabrication export, layout-quality audits, and a pile of
agent skills. It does not ship KiCad. Bring your own; he'll judge it.

## Quick start

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked

(cd frontend && trunk build)
cargo run -p backend -- serve --port 48888 --cwd /path/to/hardware/repo
# -> http://127.0.0.1:48888/
```

## CLI

```sh
kicadmium doctor --json --cwd .          # what tools/projects he found
kicadmium drc --json --cwd . --project board-a
kicadmium erc --json --cwd .
kicadmium quality --json --cwd .         # kct rules + in-house audits vs qualityProfile
kicadmium export gerbers --cwd . --out build/gerbers
kicadmium export jlcpcb  --cwd . --out build/jlcpcb
kicadmium setup --install-kicad-tools    # managed rjwalters/kicad-tools venv
kicadmium kct  --cwd . -- readiness . --format json
kicadmium tool --cwd . -- kicad-cli version
```

Environment: `KICADMIUM_KICAD_CLI`/`KICAD_CLI` pick kicad-cli,
`KICADMIUM_KCT` picks kct, `KICADMIUM_HOME` holds managed runtimes,
`KICADMIUM_LIBRARY_WORKERS` / `KICADMIUM_LIBRARY_CACHE_MB` tune library renders.

## Layout

See [AGENTS.md](AGENTS.md). Rust workspace: `backend/` (axum + CLI),
`frontend/` (Yew), `shared/` (types).

## Lineage

Ported out of the Agent Portal `kicad-pcb` plugin. Steals ideas shamelessly
from [i2cjak/Backplane](https://github.com/i2cjak/Backplane),
[rjwalters/kicad-tools](https://github.com/rjwalters/kicad-tools),
[American-Embedded/kistack](https://github.com/American-Embedded/kistack), and
[pastebom.com](https://github.com/meawoppl/pastebom.com).
