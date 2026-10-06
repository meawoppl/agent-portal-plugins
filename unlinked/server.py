"""Simulink model pane around the pinned Unlinked CLI. Model files stay authoritative."""

from __future__ import annotations

import argparse
import asyncio
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

HERE = Path(__file__).resolve().parent
REPOSITORY = "https://github.com/CosmicFrontierLabs/unlinked"
REVISION = "10378db85137f3b89e40836820862fb5a22d4e3a"
HEADER = "X-Unlinked-Request"
IGNORE = {".git", ".venv", "node_modules", ".unlinked-runs", "target", "build"}
MODEL_SUFFIXES = (".slx", ".mdl")
SOLVERS = {"euler", "rk4", "rk45"}
SIM_TIMEOUT = 120
VARIABLE = re.compile(r"[A-Za-z][A-Za-z0-9_]{0,62}=\S.*")


def executable():
    override = os.environ.get("UNLINKED_BIN")
    managed = HERE / ".tools/bin/unlinked"
    return override or (str(managed) if managed.is_file() else shutil.which("unlinked"))


def install_command():
    return [
        "cargo",
        "install",
        "--git",
        REPOSITORY,
        "--rev",
        REVISION,
        "--root",
        str(HERE / ".tools"),
        "unlinked-cli",
    ]


def require_binary():
    binary = executable()
    if not binary:
        raise ValueError("Unlinked is not installed. Run bin/unlinked setup (needs Cargo)")
    return binary


def inside(root: Path, value: str) -> Path:
    path = (root / value).resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"Path escapes project: {value}")
    return path


def discover(root: Path):
    """Model files and MATLAB scripts under root, as sorted project-relative paths."""
    models, scripts = [], []
    for directory, dirs, files in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in IGNORE and not d.startswith("."))
        for filename in files:
            relative = str((Path(directory) / filename).relative_to(root))
            if filename.lower().endswith(MODEL_SUFFIXES):
                models.append(relative)
            elif filename.lower().endswith(".m"):
                scripts.append(relative)
    return sorted(models), sorted(scripts)


def model_path(root: Path, value) -> Path:
    if not isinstance(value, str) or not value.lower().endswith(MODEL_SUFFIXES):
        raise ValueError("Expected a .slx or .mdl model path")
    path = inside(root, value)
    if not path.is_file():
        raise ValueError(f"Missing model: {value}")
    return path


def system_names(path: str) -> list[str]:
    """Split an `info` system path (`model/Outer//Slash/Inner`) into block names.

    Simulink escapes `/` inside a block name as `//`; the leading model name is
    dropped because the CLI's --system arguments start below the root.
    """
    names, current, i = [], "", 0
    while i < len(path):
        if path.startswith("//", i):
            current += "/"
            i += 2
        elif path[i] == "/":
            names.append(current)
            current = ""
            i += 1
        else:
            current += path[i]
            i += 1
    names.append(current)
    return names[1:]


def doctor(root: Path):
    binary = executable()
    models, scripts = discover(root)
    return {
        "ok": bool(binary),
        "unlinked": binary or "",
        "revision": REVISION,
        "models": models,
        "scripts": scripts,
    }


async def run_cli(binary, args, cwd: Path, timeout=SIM_TIMEOUT):
    """Run the CLI; return (exit code, stdout bytes, stderr text)."""
    process = await asyncio.create_subprocess_exec(
        binary,
        *args,
        cwd=cwd,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        start_new_session=True,
    )
    try:
        out, err = await asyncio.wait_for(process.communicate(), timeout)
    except (TimeoutError, asyncio.CancelledError):
        with suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
        await process.wait()
        raise
    return process.returncode, out, err.decode(errors="replace")


def atomic_json(path: Path, data):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(data, indent=2))
    temporary.replace(path)


class Workbench:
    def __init__(self, root: Path, session="", runs: Path | None = None):
        self.root = root.resolve()
        self.session = session
        self.runs = runs or self.root / ".unlinked-runs"
        self.lock = asyncio.Lock()

    def history(self):
        result = []
        for path in sorted(self.runs.glob("*/run.json"), reverse=True)[:50]:
            with suppress(ValueError, OSError):
                result.append(json.loads(path.read_text()))
        return result

    def run_dir(self, run_id):
        if not re.fullmatch(r"\d{8}T\d{6,12}-[0-9a-f]{8}", run_id):
            raise ValueError("Invalid run id")
        path = inside(self.runs, run_id)
        if not path.is_dir():
            raise ValueError("Run not found")
        return path

    async def info(self, model: Path):
        code, out, err = await run_cli(require_binary(), ["info", str(model)], self.root)
        if code:
            raise ValueError(err.strip() or "unlinked info failed")
        return json.loads(out)

    async def render(self, model: Path, system: str):
        args = ["render", str(model)]
        for name in system_names(system) if system else []:
            args += ["--system", name]
        code, out, err = await run_cli(require_binary(), args, self.root)
        if code:
            raise ValueError(err.strip() or "unlinked render failed")
        return out

    def sim_args(self, body: dict):
        """Validate a simulate request into CLI arguments and a record of the settings."""
        try:
            settings = {
                "start": float(body.get("start", 0)),
                "stop": float(body["stop"]),
                "step": float(body["step"]),
            }
        except (KeyError, TypeError, ValueError):
            raise ValueError("stop and step must be numbers")
        if not all(abs(v) < 1e12 for v in settings.values()):
            raise ValueError("Simulation times are out of range")
        if settings["step"] <= 0 or settings["stop"] <= settings["start"]:
            raise ValueError("step must be positive and stop must be after start")
        solver = body.get("solver", "rk4")
        if solver not in SOLVERS:
            raise ValueError("solver must be euler, rk4 or rk45")
        settings["solver"] = solver
        variables = body.get("vars", [])
        if not isinstance(variables, list) or not all(
            isinstance(v, str) and VARIABLE.fullmatch(v) for v in variables
        ):
            raise ValueError("vars must be NAME=EXPR strings")
        settings["vars"] = variables
        args = [
            "--start",
            repr(settings["start"]),
            "--stop",
            repr(settings["stop"]),
            "--step",
            repr(settings["step"]),
            "--solver",
            solver,
            "--format",
            "json",
        ]
        for variable in variables:
            args += ["--var", variable]
        return args, settings

    async def simulate(self, relative: str, body: dict):
        model = model_path(self.root, relative)
        args, settings = self.sim_args(body)
        binary = require_binary()
        run_id = (
            datetime.now(UTC).strftime("%Y%m%dT%H%M%S%f") + "-" + uuid.uuid4().hex[:8]
        )
        async with self.lock:
            directory = self.runs / run_id
            directory.mkdir(parents=True)
            record = {
                "id": run_id,
                "model": relative,
                "settings": settings,
                "status": "running",
                "started": datetime.now(UTC).isoformat(),
                "message": "",
            }
            atomic_json(directory / "run.json", record)
            trace = directory / "trace.json"
            try:
                code, out, err = await run_cli(
                    binary, ["sim", str(model), *args, "-o", str(trace)], self.root
                )
                (directory / "run.log").write_text(err)
                if code or not trace.is_file():
                    record["status"] = "failed"
                    record["message"] = err.strip().splitlines()[-1] if err.strip() else ""
                else:
                    data = json.loads(trace.read_text())
                    record["status"] = "completed"
                    record["samples"] = len(data["trace"]["time"])
                    record["signals"] = len(data["trace"]["signals"])
            except TimeoutError:
                record["status"] = "timeout"
                record["message"] = f"Simulation exceeded {SIM_TIMEOUT}s"
            record["finished"] = datetime.now(UTC).isoformat()
            atomic_json(directory / "run.json", record)
            return record


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


def make_app(work: Workbench):
    app = web.Application(middlewares=[errors], client_max_size=256 * 1024)

    async def state(request):
        return web.json_response(
            {
                **doctor(work.root),
                "project": str(work.root),
                "session": work.session,
                "runs": work.history(),
            }
        )

    async def info(request):
        model = model_path(work.root, request.query.get("file"))
        return web.json_response(await work.info(model))

    async def render(request):
        model = model_path(work.root, request.query.get("file"))
        svg = await work.render(model, request.query.get("system", ""))
        return web.Response(body=svg, content_type="image/svg+xml")

    async def simulate(request):
        body = await request.json()
        if not isinstance(body, dict):
            raise ValueError("Expected a JSON object")
        if work.lock.locked():
            return web.json_response({"error": "A simulation is already running"}, status=409)
        return web.json_response(await work.simulate(body.get("file"), body))

    async def trace(request):
        path = work.run_dir(request.match_info["run"]) / "trace.json"
        if not path.is_file():
            raise ValueError("Run has no trace")
        return web.FileResponse(path)

    async def log(request):
        path = work.run_dir(request.match_info["run"]) / "run.log"
        return web.Response(text=path.read_text(errors="replace") if path.exists() else "")

    app.router.add_get("/healthz", lambda _: web.json_response({"ok": True}))
    app.router.add_get("/api/state", state)
    app.router.add_get("/api/info", info)
    app.router.add_get("/api/render", render)
    app.router.add_post("/api/sim", simulate)
    app.router.add_get("/api/trace/{run}", trace)
    app.router.add_get("/api/log/{run}", log)
    app.router.add_get("/", lambda _: web.FileResponse(HERE / "static/index.html"))
    app.router.add_static("/static", HERE / "static")
    return app


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("serve", "doctor", "setup", "tool"))
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("--port", type=int, default=49140)
    parser.add_argument("--session", default="")
    args, extra = parser.parse_known_args()
    root = args.cwd.resolve()
    if args.command == "tool":
        if extra[:1] == ["--"]:
            extra = extra[1:]
        if not extra:
            parser.error("tool requires -- followed by unlinked arguments")
        return subprocess.call([require_binary(), *extra], cwd=root)
    if extra:
        parser.error("Unrecognized arguments: " + " ".join(extra))
    if args.command == "setup":
        if not shutil.which("cargo"):
            raise ValueError("setup needs Cargo on PATH to build the pinned unlinked CLI")
        return subprocess.call(install_command())
    if args.command == "doctor":
        report = doctor(root)
        print(json.dumps(report, indent=2))
        return 0 if report["ok"] else 1
    web.run_app(make_app(Workbench(root, args.session)), host="127.0.0.1", port=args.port)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
