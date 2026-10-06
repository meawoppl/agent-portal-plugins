"""Lifecycle and same-origin routing for the pinned upstream Visilog viewer."""

import asyncio
import os
import shutil
import signal
import socket
import uuid
from contextlib import suppress
from pathlib import Path

from aiohttp import ClientSession, ClientTimeout, web

REPOSITORY = "https://github.com/meawoppl/visilog"
REVISION = "7cf5b44d182e5c122d3f035409715c824f386ef1"
HERE = Path(__file__).resolve().parent
HEADER = "X-Visilog-Request"


def executable():
    override = os.environ.get("VISILOG_BIN")
    managed = HERE / ".tools/bin/visilog"
    return override or (str(managed) if managed.is_file() else shutil.which("visilog"))


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
        "visilog",
    ]


def adapt_asset(path, data):
    # The pinned reference client uses root-relative URLs. Keep its viewer
    # isolated under /design so the host pane keeps its own routes.
    if path == "/":
        return data.replace(b'"/viewer.', b'"/design/viewer.').replace(
            b"https://unpkg.com/elkjs@0.9.3/lib/elk.bundled.js",
            b"/design/elk.js",
        )
    if path == "/viewer.js":
        if b"fetch(path," not in data:
            raise ValueError("Visilog viewer API changed; update the pinned adapter")
        return data.replace(b"fetch(path,", b'fetch("/design" + path,').replace(
            b'headers: { "Content-Type": "application/json" }',
            b'headers: { "Content-Type": "application/json", "%s": "1" }'
            % HEADER.encode(),
        )
    if path == "/viewer.css":
        return data + (HERE / "static/design.css").read_bytes()
    return data


class DesignViewer:
    """Owns one `visilog serve` process and proxies the viewer under /design."""

    ALLOWED = {
        "/",
        "/viewer.js",
        "/viewer.css",
        "/api/design",
        "/api/values",
        "/api/traces",
        "/api/console",
        "/api/source",
        "/api/run",
        "/api/reset",
    }

    def __init__(self, runs: Path):
        self.runs = runs
        self.process = None
        self.port = None
        self.lock = asyncio.Lock()
        self.client = None
        self.directory = None

    async def close(self):
        if self.process is not None:
            with suppress(ProcessLookupError):
                os.killpg(self.process.pid, signal.SIGKILL)
            await self.process.wait()
            self.process = None
        if self.client is not None:
            await self.client.close()
            self.client = None

    async def start(self, design_args, timeout):
        """Serve a design. design_args are visilog options after `serve --port N`."""
        binary = executable()
        if not binary:
            raise ValueError("Visilog is not installed. Run bin/visilog setup-visilog")
        async with self.lock:
            await self.close()
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                self.port = sock.getsockname()[1]
            self.directory = self.runs / ("design-" + uuid.uuid4().hex)
            self.directory.mkdir(parents=True)
            args = [binary, "serve", "--port", str(self.port), "--out"]
            args += [str(self.directory), *design_args]
            log = self.directory / "viewer.log"
            with log.open("wb") as output:
                output.write(("$ " + " ".join(args) + "\n").encode())
                output.flush()
                self.process = await asyncio.create_subprocess_exec(
                    *args,
                    cwd=self.directory,
                    stdout=output,
                    stderr=output,
                    stdin=asyncio.subprocess.DEVNULL,
                    start_new_session=True,
                )
            self.client = ClientSession(timeout=ClientTimeout(total=timeout))
            try:
                async with asyncio.timeout(timeout):
                    while True:
                        if self.process.returncode is not None:
                            text = log.read_text(errors="replace")
                            raise ValueError(
                                "\n".join(text.splitlines()[1:])[-6000:]
                                or "Visilog exited"
                            )
                        try:
                            async with self.client.get(
                                self.base + "/api/design"
                            ) as response:
                                if response.status == 200:
                                    return {
                                        "url": "/design/",
                                        "revision": REVISION,
                                        "log": str(log),
                                    }
                        except OSError:
                            pass
                        await asyncio.sleep(0.1)
            except BaseException:
                await self.close()
                raise

    @property
    def base(self):
        return f"http://127.0.0.1:{self.port}"

    @property
    def running(self):
        return self.process is not None and self.process.returncode is None

    async def proxy(self, request):
        path = "/" + request.match_info["tail"]
        if path == "/elk.js":
            return web.FileResponse(HERE / "node_modules/elkjs/lib/elk.bundled.js")
        if path not in self.ALLOWED:
            raise web.HTTPNotFound()
        async with self.lock:
            if not self.running:
                raise web.HTTPServiceUnavailable(text="Load a design first")
            try:
                async with self.client.request(
                    request.method,
                    self.base + path,
                    params=request.query,
                    data=await request.read(),
                    headers={"Content-Type": "application/json"},
                ) as response:
                    data = adapt_asset(path, await response.read())
                    return web.Response(
                        body=data,
                        status=response.status,
                        headers={
                            "Content-Type": response.headers.get(
                                "Content-Type", "application/octet-stream"
                            )
                        },
                    )
            except (TimeoutError, OSError) as exc:
                await self.close()
                raise web.HTTPGatewayTimeout(
                    text="Visilog stopped responding; reload the design"
                ) from exc
