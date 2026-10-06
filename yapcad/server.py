"""yapCAD workbench: retained DSL builds, previews and a 3D review pane.

DSL sources in the project remain authoritative. Every build, package or
import becomes a run directory under .yapcad-runs/ holding a source snapshot,
the worker log, view geometry, a rendered preview and requested exports."""

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
from contextlib import suppress
from datetime import UTC, datetime
from pathlib import Path

from aiohttp import web
from watchfiles import awatch

import render

HERE = Path(__file__).resolve().parent
IGNORE = {
    ".git",
    ".venv",
    "node_modules",
    ".yapcad-runs",
    ".tools",
    "target",
    "build",
    "__pycache__",
}
SOURCES = (".dsl",)
IMPORTS = (".stl", ".step", ".stp", ".dxf")
EXPORTS = ("stl", "step", "dxf", "svg")
ARTIFACTS = {
    "view.json",
    "preview.png",
    "run.log",
    "source.dsl",
    "assembly.json",
    "result.json",
    "spec.json",
}
REPRESENTATIONS = ("mesh", "sdf")
RUN_ID = re.compile(r"\d{8}T\d{6,12}-[0-9a-f]{8}")
BREP_ENV = HERE / ".tools" / "brep"
REVISION = "5225474a23af509b95a08ae3b9815db6a32737c8"


def interpreter() -> str:
    """Python used for yapCAD work: YAPCAD_PYTHON, the managed BREP env, or ours."""
    if os.environ.get("YAPCAD_PYTHON"):
        return os.environ["YAPCAD_PYTHON"]
    managed = BREP_ENV / "bin" / "python"
    return str(managed) if managed.exists() else sys.executable


def inside(root: Path, value: str | Path) -> Path:
    path = (root / value).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"Path escapes project: {value}")
    return path


def discover(root: Path):
    """DSL sources, packages and importable geometry, relative to root."""
    found = {"sources": [], "packages": [], "imports": []}
    for directory, dirs, files in os.walk(root):
        folder = Path(directory)
        packages = [
            d
            for d in dirs
            if d.endswith(".ycpkg") and (folder / d / "manifest.yaml").is_file()
        ]
        found["packages"] += [str((folder / d).relative_to(root)) for d in packages]
        dirs[:] = sorted(
            d
            for d in dirs
            if d not in IGNORE and not d.startswith(".") and d not in packages
        )
        for name in sorted(files):
            suffix = Path(name).suffix.lower()
            relative = str((folder / name).relative_to(root))
            if suffix in SOURCES:
                found["sources"].append(relative)
            elif suffix in IMPORTS:
                found["imports"].append(relative)
    return {k: sorted(v) for k, v in found.items()}


def parse_value(text: str):
    """CLI parameter values: JSON when valid (numbers, bools, lists), else a string."""
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return text


def parse_params(pairs, raw_json=None):
    params = dict(json.loads(raw_json)) if raw_json else {}
    for pair in pairs or []:
        if "=" not in pair:
            raise ValueError(f"Parameter must be NAME=VALUE: {pair}")
        name, value = pair.split("=", 1)
        params[name.strip()] = parse_value(value.strip())
    return params


def atomic_json(path: Path, data):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(data, indent=2, default=str))
    temporary.replace(path)


def validate_spec(root: Path, body: dict, kind: str):
    if not isinstance(body, dict):
        raise ValueError("Expected a JSON object")
    path = inside(root, str(body.get("file", "")))
    if kind == "import":
        if not (path.is_file() and path.suffix.lower() in IMPORTS) and not (
            path.is_dir() and (path / "manifest.yaml").is_file()
        ):
            raise ValueError(
                "Import requires an STL, STEP, DXF file or a .ycpkg package"
            )
    elif not path.is_file() or path.suffix not in SOURCES:
        raise ValueError(f"DSL source not found: {body.get('file')}")
    spec = {
        "kind": kind,
        "file": str(path),
        "relative": str(path.relative_to(root.resolve())),
    }
    if kind != "import":
        if not isinstance(body.get("command"), str) or not re.fullmatch(
            r"[A-Za-z_]\w*", body["command"]
        ):
            raise ValueError("A command name is required")
        spec["command"] = body["command"]
        params = body.get("params") or {}
        if not isinstance(params, dict):
            raise ValueError("params must be an object")
        spec["params"] = params
        if body.get("representation") not in (None, "", *REPRESENTATIONS):
            raise ValueError("representation must be mesh or sdf")
        spec["representation"] = body.get("representation") or None
        if body.get("sdf_cell") is not None:
            spec["sdf_cell"] = float(body["sdf_cell"])
        if body.get("simplify") is not None:
            spec["simplify"] = bool(body["simplify"])
        if body.get("recursion_limit") is not None:
            spec["recursion_limit"] = int(body["recursion_limit"])
        if kind == "package":
            options = body.get("package") or {}
            spec["package"] = {
                "name": str(options.get("name") or ""),
                "version": str(options.get("version") or "1.0.0"),
                "description": options.get("description"),
                "component_exports": [
                    f
                    for f in options.get("component_exports") or []
                    if f in ("stl", "step")
                ],
            }
    exports = body.get("exports") or []
    if not isinstance(exports, list) or set(exports) - set(EXPORTS):
        raise ValueError(f"exports must be a subset of {', '.join(EXPORTS)}")
    spec["exports"] = exports
    for flag in ("strict_step", "strict_stl", "strict"):
        if body.get(flag):
            spec[flag] = True
    if body.get("step_format") in ("faceted", "analytic"):
        spec["step_format"] = body["step_format"]
    timeout = body.get("timeout", 600)
    if type(timeout) is not int or not 5 <= timeout <= 3600:
        raise ValueError("timeout must be 5..3600 seconds")
    spec["timeout"] = timeout
    return spec


def label(spec):
    if spec["kind"] == "import":
        return Path(spec["relative"]).name
    params = ", ".join(f"{k}={v}" for k, v in spec.get("params", {}).items())
    return f"{spec['command']}({params})"


async def worker(*args, timeout=120, cwd=None):
    """Run a short worker action and return its JSON output."""
    process = await asyncio.create_subprocess_exec(
        interpreter(),
        str(HERE / "worker.py"),
        *args,
        cwd=cwd,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), timeout)
    except (TimeoutError, asyncio.CancelledError):
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
        await process.wait()
        raise
    try:
        return json.loads(stdout)
    except json.JSONDecodeError:
        raise ValueError(
            (stderr or stdout).decode(errors="replace").strip()[-4000:]
            or "worker failed"
        ) from None


class Workbench:
    def __init__(self, root: Path, session="", runs: Path | None = None):
        self.root = root.resolve()
        self.session = session
        self.runs = runs or self.root / ".yapcad-runs"
        self.runs.mkdir(parents=True, exist_ok=True)
        if not (self.runs / ".gitignore").exists():
            # Runs are retained locally; keep them out of the design repository.
            (self.runs / ".gitignore").write_text("*\n")
        self.active = None
        self.task = None
        self.process = None
        self.clients = set()
        self.listing = {}
        self.capabilities = None

    async def notify(self, reason="invalidate", **extra):
        for ws in list(self.clients):
            with suppress(ConnectionError, RuntimeError):
                await ws.send_json({"type": reason, **extra})

    def history(self, limit=100):
        result = []
        for path in sorted(self.runs.glob("*/run.json"), reverse=True)[:limit]:
            with suppress(ValueError, OSError):
                result.append(json.loads(path.read_text()))
        return result

    def run_dir(self, run_id: str) -> Path:
        if run_id == "latest":
            runs = sorted(p.parent.name for p in self.runs.glob("*/run.json"))
            if not runs:
                raise ValueError("No runs yet")
            run_id = runs[-1]
        if not RUN_ID.fullmatch(run_id):
            raise ValueError("Invalid run id")
        path = inside(self.runs, run_id)
        if not path.is_dir():
            raise ValueError("Run not found")
        return path

    async def commands(self, relative: str):
        """Cached worker 'list' output for one DSL file."""
        path = inside(self.root, relative)
        key = (str(path), path.stat().st_mtime_ns)
        if key not in self.listing:
            (self.listing[key],) = await worker("list", str(path))
            for stale in [k for k in self.listing if k[0] == key[0] and k != key]:
                del self.listing[stale]
        return self.listing[key]

    async def caps(self):
        if self.capabilities is None:
            self.capabilities = await worker("capabilities", timeout=60)
        return self.capabilities

    async def execute(self, spec: dict):
        run_id = (
            datetime.now(UTC).strftime("%Y%m%dT%H%M%S%f") + "-" + uuid.uuid4().hex[:8]
        )
        directory = self.runs / run_id
        directory.mkdir()
        source = Path(spec["file"])
        digest = hashlib.sha256(
            json.dumps(
                {k: v for k, v in spec.items() if k != "timeout"}, sort_keys=True
            ).encode()
        )
        if source.is_file():
            content = source.read_bytes()
            digest.update(content)
            if spec["kind"] != "import":
                # Snapshot so later edits cannot change what this run built.
                (directory / "source.dsl").write_bytes(content)
        record = {
            "id": run_id,
            "kind": spec["kind"],
            "file": spec["relative"],
            "command": spec.get("command"),
            "params": spec.get("params", {}),
            "representation": spec.get("representation") or "mesh",
            "label": label(spec),
            "status": "running",
            "started": datetime.now(UTC).isoformat(),
            "revision": digest.hexdigest(),
            "interpreter": interpreter(),
        }
        self.active = run_id
        atomic_json(directory / "spec.json", spec)
        atomic_json(directory / "run.json", record)
        await self.notify("run", id=run_id)
        log = directory / "run.log"
        try:
            with log.open("ab") as output:
                self.process = await asyncio.create_subprocess_exec(
                    interpreter(),
                    str(HERE / "worker.py"),
                    "import" if spec["kind"] == "import" else "build",
                    "--spec",
                    str(directory / "spec.json"),
                    "--out",
                    str(directory),
                    cwd=source.parent if source.is_file() else self.root,
                    stdin=asyncio.subprocess.DEVNULL,
                    stdout=output,
                    stderr=output,
                    start_new_session=True,
                    env={**os.environ, "PYTHONUNBUFFERED": "1"},
                )
                try:
                    await asyncio.wait_for(self.process.wait(), spec["timeout"])
                finally:
                    if self.process.returncode is None:
                        with suppress(ProcessLookupError):
                            os.killpg(self.process.pid, signal.SIGKILL)
                        await self.process.wait()
                    self.process = None
            result = json.loads((directory / "result.json").read_text())
            failures = result.get("require_failures") or []
            record.update(
                {
                    k: result[k]
                    for k in (
                        "stats",
                        "exports",
                        "notes",
                        "value",
                        "metadata",
                        "package",
                        "assembly",
                        "assembly_parts",
                        "require_failures",
                        "build_seconds",
                    )
                    if k in result
                }
            )
            record["result"] = result.get("kind")
            record["params"] = result.get("params", record["params"])
            requires = "; ".join(
                " ".join(
                    filter(
                        None,
                        (f["message"], f.get("expression") and f"({f['expression']})"),
                    )
                )
                for f in failures
            )
            if not result.get("success"):
                record.update(
                    status="failed",
                    message=result.get("error") or requires or "Build failed",
                )
            elif failures:
                record.update(status="failed", message=requires)
            else:
                record["status"] = "ok"
            if (directory / "view.json").exists():
                await asyncio.to_thread(
                    render.render_file,
                    directory / "view.json",
                    directory / "preview.png",
                    title=record["label"],
                )
                record["preview"] = "preview.png"
        except TimeoutError:
            record.update(
                status="timeout", message=f"Build exceeded {spec['timeout']}s"
            )
        except asyncio.CancelledError:
            record.update(status="cancelled", message="Build cancelled")
        except (OSError, ValueError) as error:
            record.update(status="error", message=str(error))
        finally:
            record["finished"] = datetime.now(UTC).isoformat()
            record["path"] = str(directory)
            atomic_json(directory / "run.json", record)
            self.active = None
            await self.notify("run", id=run_id)
        return record

    async def cancel(self):
        if self.task and not self.task.done():
            self.task.cancel()
            with suppress(asyncio.CancelledError):
                await self.task


def artifact(directory: Path, name: str) -> Path:
    record = json.loads((directory / "run.json").read_text())
    allowed = set(ARTIFACTS) | set((record.get("exports") or {}).values())
    if name.startswith("package.ycpkg/") and record.get("package"):
        return inside(directory, name)
    if name not in allowed:
        raise ValueError("Unknown artifact")
    path = inside(directory, name)
    if not path.is_file():
        raise ValueError("Artifact unavailable")
    return path


def package_archive(directory: Path) -> Path:
    archive = directory / "package.ycpkg.zip"
    if not archive.exists():
        shutil.make_archive(
            str(directory / "package.ycpkg"), "zip", directory, "package.ycpkg"
        )
    return archive


@web.middleware
async def errors(request, handler):
    try:
        # Custom header prevents cross-origin form submissions.
        if request.method == "POST" and request.headers.get("X-Yapcad-Request") != "1":
            raise web.HTTPForbidden(text="Missing request header")
        response = await handler(request)
        response.headers["Cache-Control"] = "no-store"
        return response
    except (ValueError, KeyError, OSError, json.JSONDecodeError) as error:
        return web.json_response({"error": str(error)}, status=400)


def make_app(work: Workbench):
    app = web.Application(middlewares=[errors], client_max_size=4 * 1024 * 1024)

    async def state(request):
        try:
            caps, error = await work.caps(), None
        except (ValueError, OSError, TimeoutError) as exc:
            caps, error = {}, f"yapCAD worker unavailable: {exc}"
        return web.json_response(
            {
                **discover(work.root),
                "capabilities": caps,
                "interpreter": interpreter(),
                "error": error,
                "project": str(work.root),
                "session": work.session,
                "active": work.active,
                "runs": work.history(),
            }
        )

    async def commands(request):
        return web.json_response(await work.commands(request.query["file"]))

    async def check(request):
        body = await request.json()
        (report,) = await worker("check", str(inside(work.root, body["file"])))
        return web.json_response(report)

    async def source(request):
        path = inside(work.root, request.query["file"])
        if path.suffix not in SOURCES:
            raise ValueError("Only DSL sources are readable")
        return web.Response(text=path.read_text(errors="replace"))

    async def reference(request):
        name = request.query.get("name")
        return web.json_response(await worker("api", *([name] if name else [])))

    def starter(kind):
        async def start(request):
            body = await request.json()
            spec = validate_spec(work.root, body, kind)
            if work.task and not work.task.done():
                if not body.get("replace"):
                    return web.json_response(
                        {"error": "A build is already running"}, status=409
                    )
                await work.cancel()
            work.task = asyncio.create_task(work.execute(spec))
            return web.json_response({"accepted": True}, status=202)

        return start

    async def cancel(request):
        await work.cancel()
        return web.json_response({"ok": True})

    async def run_info(request):
        return web.FileResponse(work.run_dir(request.match_info["run"]) / "run.json")

    async def run_file(request):
        directory = work.run_dir(request.match_info["run"])
        name = request.match_info["name"]
        if name == "package.ycpkg.zip":
            path = await asyncio.to_thread(package_archive, directory)
        else:
            path = artifact(directory, name)
        response = web.FileResponse(path)
        if request.query.get("download"):
            response.headers["Content-Disposition"] = (
                f'attachment; filename="{work.run_dir(request.match_info["run"]).name}-{path.name}"'
            )
        return response

    async def rerender(request):
        directory = work.run_dir(request.match_info["run"])
        views = request.query.getall("view", ["sheet"])
        width, height = (
            int(v) for v in request.query.get("size", "960x720").split("x")
        )
        if not (64 <= width <= 4096 and 64 <= height <= 4096):
            raise ValueError("size must be within 64..4096")
        target = (
            directory
            / f"render-{hashlib.sha1(json.dumps([views, width, height]).encode()).hexdigest()[:10]}.png"
        )
        await asyncio.to_thread(
            render.render_file, directory / "view.json", target, views, (width, height)
        )
        return web.FileResponse(target)

    async def log(request):
        path = work.run_dir(request.match_info["run"]) / "run.log"
        if not path.exists():
            return web.Response(text="")
        with path.open("rb") as stream:
            stream.seek(max(0, path.stat().st_size - 256000))
            return web.Response(text=stream.read().decode(errors="replace"))

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
            path = Path(path)
            relative = (
                path.relative_to(work.root) if path.is_relative_to(work.root) else path
            )
            if path.name == "run.json":
                return path.parent.parent == work.runs
            return not any(
                p in IGNORE for p in relative.parts
            ) and path.suffix.lower() in (*SOURCES, *IMPORTS, ".yaml")

        async for changes in awatch(
            work.root, work.runs, watch_filter=relevant, debounce=600, step=100
        ):
            files = sorted(
                {
                    str(Path(p).relative_to(work.root))
                    for _, p in changes
                    if Path(p).name != "run.json" and Path(p).is_relative_to(work.root)
                }
            )
            await work.notify("files" if files else "run", files=files)

    async def lifecycle(app):
        watcher = asyncio.create_task(watch())
        yield
        watcher.cancel()
        with suppress(asyncio.CancelledError):
            await watcher
        await work.cancel()
        for ws in list(work.clients):
            await ws.close()

    app.cleanup_ctx.append(lifecycle)
    app.router.add_get("/healthz", lambda _: web.json_response({"ok": True}))
    app.router.add_get("/api/state", state)
    app.router.add_get("/api/commands", commands)
    app.router.add_get("/api/source", source)
    app.router.add_get("/api/reference", reference)
    app.router.add_post("/api/check", check)
    app.router.add_post("/api/build", starter("build"))
    app.router.add_post("/api/package", starter("package"))
    app.router.add_post("/api/import", starter("import"))
    app.router.add_post("/api/cancel", cancel)
    app.router.add_get("/api/runs/{run}", run_info)
    app.router.add_get("/api/runs/{run}/log", log)
    app.router.add_get("/api/runs/{run}/render", rerender)
    app.router.add_get("/api/runs/{run}/files/{name:.+}", run_file)
    app.router.add_get("/ws", socket)
    app.router.add_get("/", lambda _: web.FileResponse(HERE / "static/index.html"))
    app.router.add_get(
        "/lucide.js",
        lambda _: web.FileResponse(HERE / "node_modules/lucide/dist/umd/lucide.js"),
    )
    app.router.add_static("/three", HERE / "node_modules/three")
    app.router.add_static("/static", HERE / "static")
    return app


# --------------------------------------------------------------------- CLI


def doctor(root: Path):
    report = {
        "interpreter": interpreter(),
        "project": str(root),
        "viewer": (HERE / "node_modules/three/build/three.module.js").is_file(),
    }
    try:
        report["capabilities"] = asyncio.run(worker("capabilities", timeout=120))
    except (ValueError, OSError, TimeoutError) as error:
        report["capabilities"] = {}
        report["error"] = str(error)
    report.update(discover(root))
    caps = report["capabilities"]
    report["ok"] = bool(caps.get("yapcad")) and report["viewer"]
    hints = []
    if not report["viewer"]:
        hints.append("Run `bin/yapcad setup` to install the pane's three.js assets.")
    if caps and not caps.get("brep"):
        hints.append(
            "BREP (fillets of mesh solids, analytic STEP, STEP import) needs pythonocc-core: run `bin/yapcad setup-brep` (requires conda/mamba) or set YAPCAD_PYTHON to a conda Python with yapCAD installed."
        )
    report["hints"] = hints
    return report


def summarize(record):
    directory = Path(record.get("path", ""))
    summary = {
        k: record.get(k)
        for k in (
            "id",
            "status",
            "kind",
            "label",
            "file",
            "command",
            "params",
            "representation",
            "message",
            "result",
            "stats",
            "value",
            "assembly_parts",
            "notes",
            "require_failures",
            "build_seconds",
        )
        if record.get(k) not in (None, [], {})
    }
    files = {"log": str(directory / "run.log")}
    if record.get("preview"):
        files["preview"] = str(directory / record["preview"])
    for fmt, name in (record.get("exports") or {}).items():
        files[fmt] = str(directory / name)
    if record.get("package"):
        files["package"] = str(directory / "package.ycpkg")
        summary["package_valid"] = record["package"].get("valid")
        summary["package_messages"] = record["package"].get("messages")
    if record.get("assembly"):
        files["assembly"] = str(directory / record["assembly"])
    summary["files"] = files
    return summary


def setup_brep():
    manager = next(
        (shutil.which(m) for m in ("micromamba", "mamba", "conda") if shutil.which(m)),
        None,
    )
    if not manager:
        raise ValueError(
            "setup-brep needs micromamba, mamba or conda on PATH (pythonocc-core is only on conda-forge)"
        )
    code = subprocess.call(
        [
            manager,
            "create",
            "-y",
            "-p",
            str(BREP_ENV),
            "-c",
            "conda-forge",
            "python=3.12",
            "pythonocc-core>=7.7",
            "numpy",
            "pip",
        ]
    )
    if code:
        return code
    return subprocess.call(
        [
            str(BREP_ENV / "bin/python"),
            "-m",
            "pip",
            "install",
            f"yapcad[manifold,meshcheck] @ git+https://github.com/rdevaul/yapCAD@{REVISION}",
            "scipy",
        ]
    )


def main():
    actions = (
        "serve",
        "doctor",
        "setup",
        "setup-brep",
        "list",
        "check",
        "build",
        "package",
        "import",
        "render",
        "runs",
        "validate",
        "api",
        "tool",
    )
    parser = argparse.ArgumentParser(
        prog="yapcad", description="yapCAD workbench for Agent Portal"
    )
    parser.add_argument("action", choices=actions)
    parser.add_argument(
        "targets",
        nargs="*",
        help="files for list/check/validate/import, or a name for api",
    )
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("--port", type=int, default=49130)
    parser.add_argument("--session", default="")
    parser.add_argument(
        "--file", help="DSL source (build/package) or geometry file (import)"
    )
    parser.add_argument("--command", dest="dsl_command", help="DSL command to execute")
    parser.add_argument(
        "-p",
        "--param",
        action="append",
        metavar="NAME=VALUE",
        help="parameter; VALUE is JSON when valid",
    )
    parser.add_argument("--params", help="all parameters as a JSON object")
    parser.add_argument("--representation", choices=REPRESENTATIONS)
    parser.add_argument("--sdf-cell", type=float)
    parser.add_argument("--no-simplify", action="store_true")
    parser.add_argument(
        "--export",
        action="append",
        default=[],
        help="stl, step, dxf, svg (repeat or comma-separate)",
    )
    parser.add_argument("--strict-step", action="store_true")
    parser.add_argument("--strict-stl", action="store_true")
    parser.add_argument("--step-format", choices=("faceted", "analytic"))
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--name", help="package name")
    parser.add_argument("--version", default="1.0.0", help="package version")
    parser.add_argument("--description", help="package description")
    parser.add_argument("--component-export", action="append", choices=("stl", "step"))
    parser.add_argument(
        "--run", default="latest", help="run id for render (default latest)"
    )
    parser.add_argument(
        "--view",
        action="append",
        help="render view: sheet, iso, front, back, top, bottom, left, right or x,y,z",
    )
    parser.add_argument("--size", default="960x720")
    parser.add_argument(
        "--out",
        type=Path,
        help="render output path, or directory to copy build artifacts into",
    )
    parser.add_argument(
        "--strict", action="store_true", help="validate: require hashes"
    )
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument(
        "--require-brep",
        action="store_true",
        help="doctor: fail unless pythonocc-core is usable",
    )
    args, extra = parser.parse_known_args()
    root = args.cwd.resolve()
    if args.action == "tool":
        if extra[:1] == ["--"]:
            extra = extra[1:]
        # Runs Python in the yapCAD environment: `-m yapcad.dsl ...` or a script.
        return subprocess.call([interpreter(), *extra], cwd=root)
    if extra:
        parser.error("Unrecognized arguments: " + " ".join(extra))
    if args.action == "setup":
        return subprocess.call(["npm", "ci", "--ignore-scripts"], cwd=HERE)
    if args.action == "setup-brep":
        return setup_brep()
    if args.action == "doctor":
        report = doctor(root)
        print(json.dumps(report, indent=2))
        return (
            0
            if report["ok"]
            and (report["capabilities"].get("brep") or not args.require_brep)
            else 1
        )
    if args.action in ("list", "check"):
        files = [str(inside(root, t)) for t in args.targets] or [
            str(root / p) for p in discover(root)["sources"]
        ]
        if not files:
            raise ValueError("No .dsl sources found")
        report = asyncio.run(worker(args.action, *files, timeout=300))
        print(json.dumps(report, indent=2))
        return 0 if all(r["ok"] for r in report) else 1
    if args.action == "api":
        print(json.dumps(asyncio.run(worker("api", *args.targets[:1])), indent=2))
        return 0
    if args.action == "validate":
        targets = [str(inside(root, t)) for t in args.targets] or [
            str(root / p) for p in discover(root)["packages"]
        ]
        if not targets:
            raise ValueError("No .ycpkg packages found")
        report = asyncio.run(
            worker("validate", *targets, *(["--strict"] if args.strict else []))
        )
        print(json.dumps(report, indent=2))
        return 0 if all(r["valid"] for r in report) else 1
    work = Workbench(root, args.session)
    if args.action == "serve":
        web.run_app(make_app(work), host="127.0.0.1", port=args.port)
        return 0
    if args.action == "runs":
        print(json.dumps([summarize(r) for r in work.history(args.limit)], indent=2))
        return 0
    if args.action == "render":
        directory = work.run_dir(args.run)
        width, height = (int(v) for v in args.size.split("x"))
        output = args.out or directory / "render.png"
        record = json.loads((directory / "run.json").read_text())
        render.render_file(
            directory / "view.json",
            output.resolve(),
            args.view or ["sheet"],
            (width, height),
            record.get("label"),
        )
        print(str(output.resolve()))
        return 0
    file = args.file or (args.targets[0] if args.targets else None)
    if not file:
        parser.error(f"{args.action} requires --file")
    body = {
        "file": file,
        "command": args.dsl_command,
        "params": parse_params(args.param, args.params),
        "representation": args.representation,
        "sdf_cell": args.sdf_cell,
        "simplify": False if args.no_simplify else None,
        "exports": [f for item in args.export for f in item.split(",") if f],
        "strict_step": args.strict_step,
        "strict_stl": args.strict_stl,
        "step_format": args.step_format,
        "strict": args.strict,
        "timeout": args.timeout,
        "package": {
            "name": args.name,
            "version": args.version,
            "description": args.description,
            "component_exports": args.component_export,
        },
    }
    spec = validate_spec(root, body, args.action)
    record = asyncio.run(work.execute(spec))
    if args.out and record.get("path"):
        args.out.mkdir(parents=True, exist_ok=True)
        directory = Path(record["path"])
        for name in ["preview.png", *(record.get("exports") or {}).values()]:
            if (directory / name).is_file():
                shutil.copy2(directory / name, args.out / name)
        if record.get("package"):
            shutil.copytree(
                directory / "package.ycpkg",
                args.out / "package.ycpkg",
                dirs_exist_ok=True,
            )
    print(json.dumps(summarize(record), indent=2))
    return 0 if record["status"] == "ok" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
