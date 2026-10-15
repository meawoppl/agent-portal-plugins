"""Loopback-only lifecycle gateway; SlowBooks itself is served unmodified."""

import argparse
import asyncio
from contextlib import suppress
import os
from pathlib import Path
import socket

from aiohttp import ClientSession, ClientTimeout, web
from multidict import CIMultiDict

from launcher import HERE, PIN, PYTHON, SOURCE, atomic_write

HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
}
LOADING = """<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>SlowBooks · Starting</title><style>
body{background:#1a1b26;color:#c0caf5;font:16px system-ui;margin:0;min-height:100vh;
display:grid;place-items:center}main{max-width:34rem;padding:2rem}h1{color:#7aa2f7}
p{line-height:1.6}a{color:#7dcfff}</style><main><h1>SlowBooks</h1>
<p id="status" role="status" aria-live="polite">Preparing your books…</p>
<p>Your company data stays in this project. The app will open when it is ready.</p>
</main><script>
async function poll(){try{const r=await fetch('/_portal/health',{cache:'no-store'});
const s=await r.json();if(s.ready){location.reload();return}
document.querySelector('#status').textContent=s.error||s.stage;
if(s.error)return}catch(e){}setTimeout(poll,1000)}poll();</script></html>"""


def clean_headers(headers):
    extra = set(headers.get("Connection", "").lower().split(","))
    return CIMultiDict(
        (k, v)
        for k, v in headers.items()
        if k.lower() not in HOP | {s.strip() for s in extra}
    )


async def initialize(app):
    data = Path(os.environ["SLOWBOOKS_DATA_DIR"])
    log = open(data / "startup.log", "ab", buffering=0)
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)
    listener.set_inheritable(True)
    app["upstream"] = f"http://127.0.0.1:{listener.getsockname()[1]}"
    app["client"] = ClientSession(
        auto_decompress=False, timeout=ClientTimeout(total=None, sock_connect=5)
    )
    process = None

    async def command(args, *, long_running=False):
        nonlocal process
        process = await asyncio.create_subprocess_exec(
            str(PYTHON),
            *args,
            cwd=SOURCE,
            stdout=log,
            stderr=log,
            pass_fds=(listener.fileno(),) if long_running else (),
        )
        if not long_running and await process.wait() != 0:
            raise RuntimeError(
                "Initialization failed. See .slowbooks/startup.log, then restart the plugin."
            )

    async def boot():
        try:
            app["state"]["stage"] = (
                "Checking the company file and taking an upgrade backup…"
            )
            await command([str(HERE / "prepare.py")])
            app["state"]["stage"] = "Updating the company database…"
            await command(["-m", "alembic", "upgrade", "head"])
            await command(["scripts/seed_database.py"])
            company = Path(os.environ["DATABASE_URL"].removeprefix("sqlite:///"))
            atomic_write(company.with_suffix(".revision"), PIN["revision"])
            app["state"]["stage"] = "Starting SlowBooks…"
            await command(
                [
                    "-m",
                    "uvicorn",
                    "app.main:app",
                    "--fd",
                    str(listener.fileno()),
                    "--proxy-headers",
                    "--forwarded-allow-ips",
                    "127.0.0.1",
                ],
                long_running=True,
            )
            listener.close()
            for _ in range(120):
                if process.returncode is not None:
                    raise RuntimeError(
                        "SlowBooks exited. See .slowbooks/startup.log, then restart the plugin."
                    )
                try:
                    async with app["client"].get(
                        app["upstream"] + "/health", timeout=ClientTimeout(total=2)
                    ) as response:
                        if response.status == 200:
                            app["state"]["ready"] = True
                            app["state"]["stage"] = "Ready"
                            break
                except (OSError, asyncio.TimeoutError):
                    pass
                await asyncio.sleep(0.25)
            if not app["state"]["ready"]:
                raise RuntimeError(
                    "SlowBooks startup timed out. See .slowbooks/startup.log."
                )
            await process.wait()
            app["state"]["ready"] = False
            raise RuntimeError("SlowBooks stopped. Restart the plugin to continue.")
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            app["state"]["error"] = str(exc)

    task = asyncio.create_task(boot())
    yield
    task.cancel()
    with suppress(asyncio.CancelledError):
        await task
    if process and process.returncode is None:
        process.terminate()
        try:
            await asyncio.wait_for(process.wait(), timeout=10)
        except asyncio.TimeoutError:
            process.kill()
            await process.wait()
    listener.close()
    await app["client"].close()
    log.close()


async def health(request):
    return web.json_response(
        {k: request.app["state"][k] for k in ("ready", "stage", "error")},
        headers={"Cache-Control": "no-store"},
    )


async def forward(request):
    if not request.app["state"]["ready"]:
        if request.method == "GET" and request.path == "/":
            return web.Response(
                text=LOADING,
                content_type="text/html",
                headers={"Cache-Control": "no-store"},
            )
        return web.json_response(
            {"detail": request.app["state"]["error"] or request.app["state"]["stage"]},
            status=503,
            headers={"Retry-After": "2"},
        )
    async with request.app["client"].request(
        request.method,
        request.app["upstream"] + request.raw_path,
        headers=clean_headers(request.headers),
        data=request.content,
        allow_redirects=False,
    ) as upstream:
        response = web.StreamResponse(
            status=upstream.status, headers=clean_headers(upstream.headers)
        )
        await response.prepare(request)
        async for chunk in upstream.content.iter_chunked(65536):
            await response.write(chunk)
        await response.write_eof()
        return response


def create_app():
    app = web.Application(client_max_size=128 * 1024 * 1024)
    app["state"] = dict(ready=False, stage="Preparing your books…", error=None)
    app.cleanup_ctx.append(initialize)
    app.router.add_get("/_portal/health", health)
    app.router.add_route("*", "/{path:.*}", forward)
    return app


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    web.run_app(create_app(), host="127.0.0.1", port=args.port, access_log=None)
