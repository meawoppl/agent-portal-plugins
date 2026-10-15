"""Portal connector for an unmodified, pinned SlowBooks installation."""

from __future__ import annotations

import argparse
import base64
import fcntl
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request

HERE = Path(__file__).resolve().parent
PIN = json.loads((HERE / "upstream.json").read_text())
SOURCE = HERE / ".tools" / PIN["revision"]
PYTHON = HERE / ".venv/bin/python"
SECRET_NAMES = (
    "SESSION_SECRET_KEY",
    "PAYROLL_ENCRYPTION_SECRET",
    "COMPANY_FILE_ENCRYPTION_SECRET",
    "SETTINGS_ENCRYPTION_KEY",
)


def atomic_write(path, data):
    """Never expose partial state or create world-readable credentials."""
    path = Path(path)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix="." + path.name)
    try:
        with os.fdopen(fd, "wb") as file:
            file.write(data.encode() if isinstance(data, str) else data)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def data_dir(cwd):
    return Path(cwd).resolve() / ".slowbooks"


def initialize(cwd):
    root = Path(cwd).resolve()
    if not root.is_dir():
        raise ValueError(f"Project directory does not exist: {root}")
    data = data_dir(root)
    data.mkdir(mode=0o700, exist_ok=True)
    data.chmod(0o700)
    atomic_write(data / ".gitignore", "*\n")
    (data / "companies").mkdir(mode=0o700, exist_ok=True)
    marker = root / ".slowbooks-project.json"
    if not marker.exists():
        atomic_write(marker, '{"schema_version": 1}\n')
    return data


def lock_file(path):
    file = open(path, "a+")
    try:
        fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        file.close()
        raise ValueError(
            "SlowBooks is already running for this project. "
            "Use its existing pane, or stop it before restarting."
        )
    return file


def project_environment(data, port):
    keyfile = data / "secrets.json"
    if keyfile.exists():
        keys = json.loads(keyfile.read_text())
        if any(
            not isinstance(keys.get(k), str) or len(keys[k]) < 32 for k in SECRET_NAMES
        ):
            raise ValueError(
                "Invalid secrets.json; restore the original secrets backup."
            )
    else:
        if any((data / "companies").glob("*.db")):
            raise ValueError(
                "Company files exist but secrets.json is missing. "
                "Restore the original secrets; new keys cannot decrypt them."
            )
        keys = {k: secrets.token_urlsafe(48) for k in SECRET_NAMES}
        keys["SETTINGS_ENCRYPTION_KEY"] = base64.urlsafe_b64encode(
            secrets.token_bytes(32)
        ).decode()
        atomic_write(keyfile, json.dumps(keys))
    # Never inherit another project's DB, encryption keys, or desktop flags.
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith("SLOWBOOKS_") and k not in SECRET_NAMES
    }
    manifest = data / "companies.json"
    if not manifest.exists():
        atomic_write(
            manifest,
            json.dumps(
                {
                    "companies": [{"name": "My Company", "file": "company.db"}],
                    "last_opened": "company.db",
                }
            ),
        )
    company = json.loads(manifest.read_text()).get("last_opened")
    if (
        not isinstance(company, str)
        or Path(company).name != company
        or not company.endswith(".db")
        or company.startswith(".")
    ):
        raise ValueError("Invalid active company in companies.json")
    env.update(keys)
    env.update(
        DATABASE_URL="sqlite:///" + str(data / "companies" / company),
        SLOWBOOKS_DATA_DIR=str(data),
        SLOWBOOKS_ENV_FILE=str(data / "unused.env"),
        SLOWBOOKS_PRIVATE_NETWORK="1",
        APP_DEBUG="false",
        FORCE_HTTPS="false",
        APP_HOST="127.0.0.1",
        APP_PORT=str(port),
        TRUST_PROXY_HEADERS="true",
        PYTHONPATH=str(SOURCE),
        PORTAL_SLOWBOOKS_REVISION=PIN["revision"],
    )
    return env


def setup():
    uv = shutil.which("uv") or str(Path.home() / ".local/bin/uv")
    if not Path(uv).is_file():
        raise ValueError("Install uv (https://docs.astral.sh/uv/) and run setup again.")
    (HERE / ".tools").mkdir(exist_ok=True)
    with lock_file(HERE / ".tools/setup.lock"):
        subprocess.run(
            [uv, "sync", "--locked", "--no-dev", "--project", str(HERE)],
            check=True,
            stdout=sys.stderr,
        )
        if not SOURCE.exists():
            staged = Path(tempfile.mkdtemp(prefix="source-", dir=HERE / ".tools"))
            try:
                for args in (
                    ["init", "-q"],
                    ["remote", "add", "origin", PIN["repository"]],
                    ["fetch", "-q", "--depth", "1", "origin", PIN["revision"]],
                    ["checkout", "-q", "--detach", "FETCH_HEAD"],
                ):
                    subprocess.run(
                        ["git", *args], cwd=staged, check=True, stdout=sys.stderr
                    )
                staged.rename(SOURCE)
            finally:
                if staged.exists():
                    shutil.rmtree(staged)
        actual = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=SOURCE, text=True
        ).strip()
        subprocess.run(
            ["git", "diff", "--exit-code", "--quiet", "HEAD"], cwd=SOURCE, check=True
        )
        if actual != PIN["revision"]:
            raise ValueError(
                "Managed SlowBooks source revision does not match the pin."
            )
        subprocess.run(
            [str(PYTHON), "-c", "import fastapi, sqlalchemy, weasyprint, sqlcipher3"],
            check=True,
            stdout=sys.stderr,
        )
    print(json.dumps({"ok": True, **PIN, "source": str(SOURCE)}))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None  # Never send an API token to a redirected host.


def request(url, *, method="GET", headers=None, body=None, timeout=30):
    opener = urllib.request.build_opener(NoRedirect(), urllib.request.ProxyHandler({}))
    req = urllib.request.Request(url, data=body, headers=headers or {}, method=method)
    try:
        response = opener.open(req, timeout=timeout)
    except urllib.error.HTTPError as exc:
        response = exc
    with response:
        return response.status, response.headers, response.read()


def running_state(data):
    statefile = data / "service.json"
    if not statefile.exists():
        raise ValueError(
            "Start SlowBooks with agent-portal plugin open slowbooks first."
        )
    try:
        with lock_file(data / "service.lock"):
            raise ValueError("SlowBooks is stopped; open the plugin to restart it.")
    except ValueError as exc:
        if "already running" not in str(exc):
            raise
    state = json.loads(statefile.read_text())
    port = state.get("port")
    if not isinstance(port, int) or not 1 <= port <= 65535:
        raise ValueError("Invalid service state")
    return state


def doctor(cwd):
    data = data_dir(cwd)
    report = {
        "ok": SOURCE.is_dir() and PYTHON.is_file(),
        **PIN,
        "source": str(SOURCE),
        "data_directory": str(data),
        "running": False,
        "api_token_configured": bool(os.environ.get("SLOWBOOKS_API_TOKEN"))
        or (data / "api-token").is_file(),
    }
    try:
        state = running_state(data)
        status, _, body = request(f"http://127.0.0.1:{state['port']}/health", timeout=3)
        report.update(
            running=status == 200, port=state["port"], health=json.loads(body)
        )
    except (ValueError, OSError, urllib.error.URLError):
        pass
    print(json.dumps(report))
    return 0 if report["ok"] else 1


def serve(cwd, port, session):
    if not SOURCE.is_dir() or not PYTHON.is_file():
        raise ValueError("SlowBooks is not installed; run bin/slowbooks setup first.")
    if not 1 <= port <= 65535:
        raise ValueError("Port must be between 1 and 65535")
    data = initialize(cwd)
    service_lock = lock_file(data / "service.lock")
    # Keep the project lock through exec and let the OS release it on exit.
    os.set_inheritable(service_lock.fileno(), True)
    env = project_environment(data, port)
    atomic_write(
        data / "service.json",
        json.dumps(
            {
                "pid": os.getpid(),
                "port": port,
                "session": session,
                "revision": PIN["revision"],
            }
        ),
    )
    os.execve(
        str(PYTHON), [str(PYTHON), str(HERE / "surface.py"), "--port", str(port)], env
    )


def api(args):
    path = urllib.parse.urlsplit(args.path)
    if path.scheme or path.netloc or not path.path.startswith("/api/") or path.fragment:
        raise ValueError("Use an API path such as /api/accounts, not an external URL.")
    state = running_state(data_dir(args.cwd))
    token = os.environ.get("SLOWBOOKS_API_TOKEN", "").strip()
    tokenfile = data_dir(args.cwd) / "api-token"
    if not token and tokenfile.is_file():
        token = tokenfile.read_text().strip()
    if not token:
        raise ValueError(
            "Create a scoped API token in SlowBooks Settings, then pass it "
            "to bin/slowbooks token on stdin or set SLOWBOOKS_API_TOKEN."
        )
    if any(c.isspace() for c in token):
        raise ValueError("API token must be a single token without whitespace.")
    headers = {"Authorization": "Bearer " + token, "Accept": "application/json"}
    body = None
    if args.json_file:
        raw = (
            sys.stdin.read()
            if args.json_file == "-"
            else Path(args.json_file).read_text()
        )
        body = json.dumps(json.loads(raw)).encode()
        headers["Content-Type"] = "application/json"
        if args.method == "GET":
            raise ValueError("A JSON body requires an explicit --method")
    if args.idempotency_key:
        headers["Idempotency-Key"] = args.idempotency_key
    status, _, result = request(
        f"http://127.0.0.1:{state['port']}" + args.path,
        method=args.method,
        headers=headers,
        body=body,
    )
    if args.output and 200 <= status < 300:
        atomic_write(Path(args.output).resolve(), result)
        print(
            json.dumps({"status": status, "output": str(Path(args.output).resolve())})
        )
    else:
        try:
            decoded = json.loads(result)
        except (ValueError, UnicodeDecodeError):
            raise ValueError("Non-JSON response; use --output for PDF/CSV/downloads.")
        print(json.dumps({"status": status, "body": decoded}))
    return 0 if 200 <= status < 300 else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("setup")
    for name in ("doctor", "serve", "api", "token"):
        p = commands.add_parser(name)
        p.add_argument("--cwd", default=os.getcwd())
        if name == "serve":
            p.add_argument("--port", type=int, required=True)
            p.add_argument("--session", default="")
        if name == "api":
            p.add_argument("path")
            p.add_argument(
                "--method",
                default="GET",
                choices=["GET", "POST", "PUT", "PATCH", "DELETE"],
            )
            p.add_argument("--json-file")
            p.add_argument("--output")
            p.add_argument("--idempotency-key")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if args.command == "setup":
            setup()
        elif args.command == "doctor":
            return doctor(args.cwd)
        elif args.command == "serve":
            serve(args.cwd, args.port, args.session)
        elif args.command == "api":
            return api(args)
        elif args.command == "token":
            token = sys.stdin.read().strip()
            if not token or any(c.isspace() for c in token):
                raise ValueError("Provide one API token on standard input.")
            atomic_write(initialize(args.cwd) / "api-token", token)
            print('{"ok": true, "token_saved": true}')
    except (
        ValueError,
        OSError,
        subprocess.CalledProcessError,
        urllib.error.URLError,
    ) as exc:
        print(
            json.dumps(
                {
                    "ok": False,
                    "error": str(exc),
                    "hint": "For startup failures, inspect .slowbooks/startup.log.",
                }
            ),
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
