"""Interactive design pane around the pinned Visilog viewer. Sources stay authoritative."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path

from aiohttp import web

from visilog_bridge import HEADER, REVISION, DesignViewer, executable, install_command

HERE = Path(__file__).resolve().parent
IGNORE = {
    ".git",
    ".venv",
    "node_modules",
    ".verilog-runs",
    ".visilog-runs",
    "target",
    "build",
}


def inside(root: Path, value: str) -> Path:
    path = (root / value).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"Path escapes project: {value}")
    return path


def strings(value, field):
    if not isinstance(value, list) or not all(isinstance(x, str) for x in value):
        raise ValueError(f"{field} must be an array of strings")
    return value


@dataclass
class Bench:
    id: str
    top: str
    sources: list[str]
    includes: list[str]
    defines: list[str]
    plusargs: list[str]
    timeout: int = 60


def discover(root: Path) -> list[Bench]:
    """Bench definitions: explicit `.verilog-workbench.json` or `_tb` discovery."""
    config = root / ".verilog-workbench.json"
    if config.exists():
        data = json.loads(config.read_text())
        if not isinstance(data, dict) or set(data) - {"version", "tests"}:
            raise ValueError("Config must contain only version and tests")
        if data.get("version") != 1 or not isinstance(data.get("tests"), list):
            raise ValueError("Config requires version: 1 and tests: []")
        result = []
        for item in data["tests"]:
            if not isinstance(item, dict) or set(item) - set(Bench.__annotations__):
                raise ValueError("Unknown test field")
            if not all(isinstance(item.get(k), str) and item[k] for k in ("id", "top")):
                raise ValueError("Each test needs nonempty id and top")
            args = {
                k: strings(item.get(k, []), k)
                for k in ("sources", "includes", "defines", "plusargs")
            }
            if not args["sources"]:
                raise ValueError("Each test needs source files (no implicit globs)")
            for field in ("sources", "includes"):
                for value in args[field]:
                    path = inside(root, value)
                    if not (path.is_file() if field == "sources" else path.is_dir()):
                        raise ValueError(f"Missing {field}: {value}")
            if not all(x.startswith("+") for x in args["plusargs"]):
                raise ValueError("Simulation plusargs must start with +")
            timeout = item.get("timeout", 60)
            if type(timeout) is not int or not 1 <= timeout <= 600:
                raise ValueError("timeout must be 1..600 seconds")
            result.append(Bench(item["id"], item["top"], **args, timeout=timeout))
        if len({b.id for b in result}) != len(result):
            raise ValueError("Duplicate test id")
        return result
    benches = []
    for directory, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in IGNORE and not d.startswith(".")]
        folder = Path(directory)
        for filename in sorted(files):
            if not filename.endswith(("_tb.v", "_tb.sv")):
                continue
            rtl = (
                folder.with_name(folder.name[:-5])
                if folder.name.endswith("_test")
                else folder
            )
            sources = sorted(
                p
                for p in rtl.glob("*")
                if p.suffix in (".v", ".sv") and not p.stem.endswith("_tb")
            )
            bench = folder / filename
            sources.append(bench)
            benches.append(
                Bench(
                    str(bench.relative_to(root)),
                    bench.stem,
                    [str(p.relative_to(root)) for p in sources],
                    [str(rtl.relative_to(root))],
                    [],
                    [],
                )
            )
    return sorted(benches, key=lambda b: b.id)


def cells_sim(root: Path) -> Path | None:
    candidates = [
        root / "fpga/third_party/yosys/cells_sim.v",
        root / "third_party/yosys/cells_sim.v",
    ]
    if os.environ.get("VERILOG_CELLS_SIM"):
        candidates.insert(0, Path(os.environ["VERILOG_CELLS_SIM"]))
    candidates += [
        Path(p)
        for p in (
            "/usr/share/yosys/ice40/cells_sim.v",
            "/usr/local/share/yosys/ice40/cells_sim.v",
        )
    ]
    return next((p for p in candidates if p.is_file()), None)


def design_args(root: Path, bench: Bench) -> list[str]:
    """Visilog options shared by serve, run and graph for one bench."""
    args = ["-s", bench.top, "--search", str(root)]
    for include in bench.includes:
        args += ["-I", str(inside(root, include))]
    for define in bench.defines:
        args += ["-D", define]
    args += bench.plusargs
    args += [str(inside(root, source)) for source in bench.sources]
    cells = cells_sim(root)
    if cells and str(cells) not in args:
        args.append(str(cells))
    return args


def select(root: Path, test_id) -> Bench:
    bench = next((b for b in discover(root) if b.id == test_id), None)
    if bench is None:
        raise ValueError("Unknown test")
    return bench


def doctor(root: Path):
    binary = executable()
    return {
        "ok": bool(binary),
        "visilog": binary or "",
        "revision": REVISION,
        "cells_sim": str(cells_sim(root) or ""),
        "tests": [asdict(b) for b in discover(root)],
    }


@web.middleware
async def errors(request, handler):
    try:
        # Custom header prevents cross-origin form submissions.
        if request.method == "POST" and request.headers.get(HEADER) != "1":
            raise web.HTTPForbidden(text="Missing request header")
        response = await handler(request)
        response.headers["Cache-Control"] = "no-store"
        return response
    except (ValueError, KeyError, OSError) as error:
        return web.json_response({"error": str(error)}, status=400)


def make_app(root: Path, session="", runs: Path | None = None):
    root = root.resolve()
    design = DesignViewer(runs or root / ".visilog-runs")
    app = web.Application(middlewares=[errors], client_max_size=2 * 1024 * 1024)
    loaded = {"test": None}

    async def state(request):
        try:
            info = doctor(root)
            error = None
        except ValueError as exc:
            info, error = {"tests": [], "ok": False, "visilog": ""}, str(exc)
        return web.json_response(
            {
                **info,
                "error": error,
                "project": str(root),
                "session": session,
                "loaded": loaded["test"] if design.running else None,
            }
        )

    async def open_design(request):
        body = await request.json()
        if not isinstance(body, dict):
            return web.json_response({"error": "Expected a JSON object"}, status=400)
        bench = select(root, body.get("test"))
        try:
            result = await design.start(design_args(root, bench), bench.timeout)
        except TimeoutError:
            raise ValueError("Visilog design setup timed out")
        loaded["test"] = bench.id
        return web.json_response({**result, "test": bench.id})

    async def lifecycle(app):
        yield
        await design.close()

    app.cleanup_ctx.append(lifecycle)
    app.router.add_get("/healthz", lambda _: web.json_response({"ok": True}))
    app.router.add_get("/api/state", state)
    app.router.add_post("/api/design/open", open_design)
    app.router.add_route("*", "/design/{tail:.*}", design.proxy)
    app.router.add_get("/", lambda _: web.FileResponse(HERE / "static/index.html"))
    app.router.add_static("/static", HERE / "static")
    return app


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "command", choices=("serve", "doctor", "graph", "setup", "setup-visilog", "tool")
    )
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("--port", type=int, default=49130)
    parser.add_argument("--session", default="")
    parser.add_argument("--test")
    args, extra = parser.parse_known_args()
    root = args.cwd.resolve()
    if args.command == "tool":
        if extra[:1] == ["--"]:
            extra = extra[1:]
        if not extra:
            parser.error("tool requires -- followed by visilog arguments")
        binary = executable()
        if not binary:
            raise ValueError("Visilog is not installed. Run bin/visilog setup-visilog")
        return subprocess.call([binary, *extra], cwd=root)
    if extra:
        parser.error("Unrecognized arguments: " + " ".join(extra))
    if args.command == "setup":
        return subprocess.call(["npm", "ci", "--ignore-scripts"], cwd=HERE)
    if args.command == "setup-visilog":
        return subprocess.call(install_command())
    if args.command == "doctor":
        report = doctor(root)
        print(json.dumps(report, indent=2))
        return 0 if report["ok"] else 1
    if args.command == "graph":
        benches = discover(root)
        if args.test is None and len(benches) != 1:
            raise ValueError("graph needs --test ID; doctor lists the discovered tests")
        bench = benches[0] if args.test is None else select(root, args.test)
        binary = executable()
        if not binary:
            raise ValueError("Visilog is not installed. Run bin/visilog setup-visilog")
        return subprocess.call([binary, "graph", *design_args(root, bench)], cwd=root)
    web.run_app(make_app(root, args.session), host="127.0.0.1", port=args.port)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
