"""Local simulation workbench. Source repositories remain authoritative."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import uuid
from bisect import bisect_right
from collections import OrderedDict
from contextlib import suppress
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path

from aiohttp import web
from vcdvcd import VCDVCD
from watchfiles import awatch

HERE = Path(__file__).resolve().parent
TOOLS = ("iverilog", "vvp", "verilator", "yosys", "nextpnr-ice40", "icepack", "fst2vcd")
IGNORE = {".git", ".venv", "node_modules", ".verilog-runs", "target", "build"}
MAX_VCD = 64 * 1024 * 1024


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


def doctor(root: Path):
    tools = {name: shutil.which(name) for name in TOOLS}
    return {
        "ok": bool(tools["iverilog"] and tools["vvp"]),
        "tools": tools,
        "cells_sim": str(cells_sim(root) or ""),
        "tests": [asdict(b) for b in discover(root)],
    }


def failed_log(text: str) -> bool:
    # Legacy benches use $display("ERROR: ...") then $finish (exit code zero).
    text = "\n".join(line for line in text.splitlines() if not line.startswith("$ "))
    return bool(re.search(r"\b(?:error|fatal|fail(?:ed|ure)?)\b", text, re.IGNORECASE))


def atomic_json(path: Path, data):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(data, indent=2))
    temporary.replace(path)


class Workbench:
    def __init__(self, root: Path, session="", runs: Path | None = None):
        self.root = root.resolve()
        self.session = session
        self.runs = runs or self.root / ".verilog-runs"
        self.runs.mkdir(parents=True, exist_ok=True)
        self.active = None
        self.task = None
        self.process = None
        self.clients = set()
        self.cache = OrderedDict()
        self.parse_lock = asyncio.Lock()

    async def notify(self):
        for ws in list(self.clients):
            with suppress(ConnectionError):
                await ws.send_json({"type": "invalidate"})

    def history(self):
        result = []
        for path in sorted(self.runs.glob("*/run.json"), reverse=True)[:100]:
            with suppress(ValueError, OSError):
                result.append(json.loads(path.read_text()))
        return result

    async def command(self, args, directory: Path, timeout: int, log: Path):
        with log.open("ab") as output:
            output.write(("$ " + json.dumps(args) + "\n").encode())
            output.flush()
            self.process = await asyncio.create_subprocess_exec(
                *args,
                cwd=directory,
                stdin=asyncio.subprocess.DEVNULL,
                stdout=output,
                stderr=output,
                start_new_session=True,
            )
            try:
                return await asyncio.wait_for(self.process.wait(), timeout)
            except (TimeoutError, asyncio.CancelledError):
                with suppress(ProcessLookupError):
                    os.killpg(self.process.pid, signal.SIGKILL)
                await self.process.wait()
                raise
            finally:
                self.process = None

    async def run(self, bench: Bench):
        run_id = (
            datetime.now(UTC).strftime("%Y%m%dT%H%M%S%f") + "-" + uuid.uuid4().hex[:8]
        )
        directory = self.runs / run_id
        directory.mkdir()
        log = directory / "run.log"
        result = {
            "id": run_id,
            "test": bench.id,
            "top": bench.top,
            "status": "running",
            "started": datetime.now(UTC).isoformat(),
            "waves": [],
            "message": "",
        }
        self.active = run_id
        atomic_json(directory / "run.json", result)
        await self.notify()
        try:
            sources = [inside(self.root, p) for p in bench.sources]
            cells = cells_sim(self.root)
            if cells and cells not in sources:
                sources.append(cells)
            digest = hashlib.sha256(json.dumps(asdict(bench), sort_keys=True).encode())
            # Snapshot inputs so later edits cannot change what this run compiled.
            snapshot = directory / "sources"
            snapshot.mkdir()
            local_sources = []
            for index, source in enumerate(sources):
                content = source.read_bytes()
                digest.update(str(source).encode() + b"\0" + content)
                dest = snapshot / (
                    str(source.relative_to(self.root))
                    if source.is_relative_to(self.root)
                    else f"vendor/{index}-{source.name}"
                )
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_bytes(content)
                local_sources.append(str(dest))
            includes = []
            for include in bench.includes:
                original = inside(self.root, include)
                target = snapshot / include
                target.mkdir(parents=True, exist_ok=True)
                for directory_name, dirs, files in os.walk(original):
                    dirs[:] = [
                        d for d in dirs if d not in IGNORE and not d.startswith(".")
                    ]
                    for filename in sorted(files):
                        header = Path(directory_name) / filename
                        if header.suffix not in (".vh", ".svh"):
                            continue
                        inside(self.root, str(header.relative_to(self.root)))
                        content = header.read_bytes()
                        digest.update(str(header).encode() + b"\0" + content)
                        dest = target / header.relative_to(original)
                        dest.parent.mkdir(parents=True, exist_ok=True)
                        dest.write_bytes(content)
                includes += ["-I", str(target)]
            result["revision"] = digest.hexdigest()
            result["config"] = asdict(bench)
            compiler = shutil.which("iverilog")
            runtime = shutil.which("vvp")
            if not compiler or not runtime:
                raise ValueError(
                    "Install Icarus Verilog (iverilog and vvp), then rerun doctor"
                )
            args = [
                compiler,
                "-g2012",
                "-gassertions",
                "-s",
                bench.top,
                "-o",
                str(directory / "sim.vvp"),
                *includes,
                *["-D" + d for d in bench.defines],
                *local_sources,
            ]
            code = await self.command(args, directory, bench.timeout, log)
            result["compile_exit"] = code
            if code:
                result["status"] = "compile-failed"
            else:
                code = await self.command(
                    [runtime, str(directory / "sim.vvp"), *bench.plusargs],
                    directory,
                    bench.timeout,
                    log,
                )
                result["simulation_exit"] = code
                result["status"] = (
                    "failed"
                    if code or failed_log(log.read_text(errors="replace"))
                    else "passed"
                )
        except TimeoutError:
            result.update(
                status="timeout", message=f"Process exceeded {bench.timeout}s"
            )
        except asyncio.CancelledError:
            result.update(status="cancelled", message="Run cancelled")
        except (OSError, ValueError) as error:
            result.update(status="error", message=str(error))
        finally:
            result["waves"] = [
                str(p.relative_to(directory)) for p in sorted(directory.rglob("*.vcd"))
            ]
            result["finished"] = datetime.now(UTC).isoformat()
            atomic_json(directory / "run.json", result)
            self.active = None
            await self.notify()
        return result

    def run_dir(self, run_id):
        if not re.fullmatch(r"\d{8}T\d{6,12}-[0-9a-f]{8}", run_id):
            raise ValueError("Invalid run id")
        path = inside(self.runs, run_id)
        if not path.is_dir():
            raise ValueError("Run not found")
        return path

    async def wave(self, run_id, filename):
        directory = self.run_dir(run_id)
        metadata = json.loads((directory / "run.json").read_text())
        if metadata["status"] == "running" or filename not in metadata["waves"]:
            raise ValueError("Waveform is not from a completed run")
        path = inside(directory, filename)
        if path.stat().st_size > MAX_VCD:
            raise ValueError(
                "VCD exceeds 64 MiB viewer limit; use GTKWave or narrow the dump scope"
            )
        key = str(path)
        async with self.parse_lock:
            if key not in self.cache:
                parsed = await asyncio.to_thread(VCDVCD, str(path), store_tvs=True)
                if parsed.endtime > 2**53 - 1:
                    raise ValueError(
                        "Time range exceeds browser integer precision; inspect in GTKWave"
                    )
                self.cache[key] = parsed
                while len(self.cache) > 2:
                    self.cache.popitem(last=False)
            return self.cache[key]


@web.middleware
async def errors(request, handler):
    try:
        # Custom header prevents cross-origin form submissions.
        if request.method == "POST" and request.headers.get("X-Verilog-Request") != "1":
            raise web.HTTPForbidden(text="Missing request header")
        response = await handler(request)
        response.headers["Cache-Control"] = "no-store"
        return response
    except (ValueError, KeyError, OSError) as error:
        return web.json_response({"error": str(error)}, status=400)


def make_app(work: Workbench):
    app = web.Application(middlewares=[errors], client_max_size=2 * 1024 * 1024)

    async def state(request):
        try:
            info = doctor(work.root)
            error = None
        except ValueError as exc:
            info, error = {"tests": [], "tools": {}}, str(exc)
        return web.json_response(
            {
                **info,
                "error": error,
                "project": str(work.root),
                "session": work.session,
                "active": work.active,
                "runs": work.history(),
            }
        )

    async def start(request):
        if work.task and not work.task.done():
            return web.json_response(
                {"error": "A simulation is already running"}, status=409
            )
        body = await request.json()
        if not isinstance(body, dict):
            return web.json_response({"error": "Expected a JSON object"}, status=400)
        bench = next((b for b in discover(work.root) if b.id == body.get("test")), None)
        if not bench:
            raise ValueError("Unknown test")
        work.task = asyncio.create_task(work.run(bench))
        return web.json_response({"accepted": True}, status=202)

    async def cancel(request):
        if work.task and not work.task.done():
            work.task.cancel()
            with suppress(asyncio.CancelledError):
                await work.task
        return web.json_response({"ok": True})

    async def waveform(request):
        wave = await work.wave(request.query["run"], request.query["file"])
        names = request.query.getall("signal", [])
        if not names:
            return web.json_response(
                {
                    "end": wave.endtime,
                    "timescale": str(wave.timescale["timescale"]),
                    "signals": [
                        {
                            "name": n,
                            "width": int(wave[n].size),
                            "type": wave[n].var_type,
                        }
                        for n in wave.signals
                    ],
                }
            )
        if len(names) > 64:
            raise ValueError("Select at most 64 signals")
        start_time = max(0, int(request.query.get("start", 0)))
        end = min(wave.endtime, int(request.query.get("end", wave.endtime)))
        if end < start_time:
            raise ValueError("Invalid time range")
        traces = []
        for name in names:
            signal_data = wave[name]
            events = signal_data.tv
            left = max(0, bisect_right(events, start_time, key=lambda x: x[0]) - 1)
            right = bisect_right(events, end, key=lambda x: x[0])
            if right - left > 100000:
                raise ValueError(
                    f"Too many transitions in {name}; zoom in to inspect this range"
                )
            traces.append(
                {
                    "name": name,
                    "width": int(signal_data.size),
                    "type": signal_data.var_type,
                    "events": events[left:right],
                }
            )
        return web.json_response({"traces": traces})

    async def log(request):
        path = work.run_dir(request.match_info["run"]) / "run.log"
        if not path.exists():
            return web.Response(text="")
        with path.open("rb") as stream:
            stream.seek(max(0, path.stat().st_size - 256000))
            content = stream.read().decode(errors="replace")
        return web.Response(text=content)

    async def download(request):
        directory = work.run_dir(request.match_info["run"])
        info = json.loads((directory / "run.json").read_text())
        name = request.query["file"]
        if name not in info["waves"] or info["status"] == "running":
            raise ValueError("Waveform unavailable")
        return web.FileResponse(inside(directory, name))

    async def socket(request):
        ws = web.WebSocketResponse(heartbeat=15)
        await ws.prepare(request)
        work.clients.add(ws)
        try:
            async for _ in ws:
                pass
        finally:
            work.clients.discard(ws)
        return ws

    async def watch():
        def relevant(change, path):
            relative = Path(path).relative_to(work.root)
            return not any(p in IGNORE for p in relative.parts) and (
                Path(path).suffix in (".v", ".sv", ".vh", ".svh")
                or Path(path).name == ".verilog-workbench.json"
            )

        async for _ in awatch(
            work.root, watch_filter=relevant, debounce=2000, step=200
        ):
            await work.notify()

    async def lifecycle(app):
        watcher = asyncio.create_task(watch())
        yield
        watcher.cancel()
        with suppress(asyncio.CancelledError):
            await watcher
        if work.task and not work.task.done():
            work.task.cancel()
            with suppress(asyncio.CancelledError):
                await work.task
        for ws in list(work.clients):
            await ws.close()

    app.cleanup_ctx.append(lifecycle)
    app.router.add_get("/healthz", lambda _: web.json_response({"ok": True}))
    app.router.add_get("/api/state", state)
    app.router.add_post("/api/run", start)
    app.router.add_post("/api/cancel", cancel)
    app.router.add_get("/api/wave", waveform)
    app.router.add_get("/api/log/{run}", log)
    app.router.add_get("/api/download/{run}", download)
    app.router.add_get("/ws", socket)
    app.router.add_get("/", lambda _: web.FileResponse(HERE / "static/index.html"))
    app.router.add_get(
        "/lucide.js",
        lambda _: web.FileResponse(HERE / "node_modules/lucide/dist/umd/lucide.js"),
    )
    app.router.add_static("/static", HERE / "static")
    return app


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("serve", "test", "doctor", "setup", "tool"))
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("--port", type=int, default=49120)
    parser.add_argument("--session", default="")
    parser.add_argument("--test")
    args, extra = parser.parse_known_args()
    root = args.cwd.resolve()
    if args.command == "tool":
        if extra[:1] == ["--"]:
            extra = extra[1:]
        if not extra or extra[0] not in TOOLS:
            parser.error("tool requires -- followed by an allowed HDL executable")
        return subprocess.call(extra, cwd=root)
    if extra:
        parser.error("Unrecognized arguments: " + " ".join(extra))
    if args.command == "setup":
        return subprocess.call(["npm", "ci", "--ignore-scripts"], cwd=HERE)
    if args.command == "doctor":
        report = doctor(root)
        print(json.dumps(report, indent=2))
        return 0 if report["ok"] else 1
    work = Workbench(root, args.session)
    if args.command == "serve":
        web.run_app(make_app(work), host="127.0.0.1", port=args.port)
        return 0
    benches = [b for b in discover(root) if not args.test or b.id == args.test]
    if not benches:
        raise ValueError("No matching tests found")

    async def suite():
        return [await work.run(bench) for bench in benches]

    results = asyncio.run(suite())
    print(json.dumps(results, indent=2))
    return 0 if all(r["status"] == "passed" for r in results) else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
