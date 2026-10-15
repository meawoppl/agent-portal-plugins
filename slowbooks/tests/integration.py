"""Real pinned-app lifecycle + browser smoke, using only temporary books."""

import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import time

import httpx
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / "bin/slowbooks"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def start(project, log):
    port = free_port()
    proc = subprocess.Popen(
        [str(CLI), "serve", "--cwd", str(project), "--port", str(port)],
        stdout=log,
        stderr=log,
    )
    base = f"http://127.0.0.1:{port}"
    for _ in range(40):
        try:
            response = httpx.get(base + "/_portal/health", timeout=1)
            if response.status_code == 200:
                break
        except httpx.HTTPError:
            pass
        assert proc.poll() is None, "surface exited before health"
        time.sleep(0.1)
    else:
        stop(proc)
        raise AssertionError("surface missed Portal health deadline")
    return proc, base


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=15)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def cli(project, *args, input=None, expected=0):
    result = subprocess.run(
        [str(CLI), *args, "--cwd", str(project)],
        input=input,
        text=True,
        capture_output=True,
    )
    assert result.returncode == expected, result.stderr
    return json.loads(result.stdout)


def main():
    with tempfile.TemporaryDirectory(prefix="slowbooks-integration-") as temporary:
        project = Path(temporary)
        with open(project / "service.log", "w") as log:
            proc, base = start(project, log)
            try:
                # A real browser sees the loading page, then upstream first-run UI.
                with sync_playwright() as pw:
                    browser = pw.chromium.launch()
                    page = browser.new_page(viewport={"width": 1440, "height": 1000})
                    errors = []
                    page.on("pageerror", lambda exc: errors.append(str(exc)))
                    page.goto(base)
                    page.locator("#auth-form").wait_for(timeout=180000)
                    password = secrets.token_urlsafe(24)
                    page.locator("#operator_name").fill("Plugin Test")
                    page.locator("#operator_email").fill("test@example.invalid")
                    page.locator("#company_name").fill("Portal Test Books")
                    page.locator("#auth-password").fill(password)
                    page.locator("#auth-password-confirm").fill(password)
                    page.locator("#auth-submit").click()
                    page.locator("#auth-form").wait_for(state="detached", timeout=30000)
                    page.wait_for_timeout(1500)
                    notice = page.get_by_role("button", name="I understand", exact=True)
                    if notice.is_visible():
                        notice.click()
                    page.wait_for_timeout(300)
                    assert not errors, errors
                    artifacts = Path(
                        os.environ.get(
                            "SLOWBOOKS_TEST_ARTIFACTS", "/tmp/slowbooks-smoke"
                        )
                    )
                    artifacts.mkdir(parents=True, exist_ok=True)
                    page.screenshot(path=str(artifacts / "desktop.png"), full_page=True)
                    page.set_viewport_size({"width": 390, "height": 844})
                    page.screenshot(path=str(artifacts / "mobile.png"), full_page=True)
                    response = page.request.post(
                        base + "/api/tokens",
                        data={"label": "plugin-readonly", "role": "readonly"},
                    )
                    assert response.status == 201, response.text()
                    token = response.json()["token"]
                    cli(project, "token", input=token)
                    accounts = cli(project, "api", "/api/accounts")
                    assert accounts["status"] == 200
                    payload = project / "customer.json"
                    payload.write_text(json.dumps({"name": "Persisted Customer"}))
                    forbidden = cli(
                        project,
                        "api",
                        "/api/customers",
                        "--method",
                        "POST",
                        "--json-file",
                        str(payload),
                        expected=1,
                    )
                    assert forbidden["status"] == 403
                    created = page.request.post(
                        base + "/api/customers", data={"name": "Persisted Customer"}
                    )
                    assert created.status == 201, created.text()
                    result = cli(
                        project,
                        "api",
                        "/api/reports/profit-loss/pdf",
                        "--output",
                        str(project / "report.pdf"),
                    )
                    assert result["status"] == 200
                    assert (project / "report.pdf").read_bytes().startswith(b"%PDF")
                    encrypted = page.request.post(
                        base + "/api/company-file/encryption",
                        data={"enabled": True, "company_name": "Portal Test Books"},
                    )
                    assert encrypted.status == 200, encrypted.text()
                    assert encrypted.json()["encrypted"]
                    # Secrets survive; simulate an upstream revision change to exercise backup.
                    keys = (project / ".slowbooks/secrets.json").read_bytes()
                    browser.close()
                stop(proc)
                (project / ".slowbooks/companies/company.revision").write_text(
                    "older-revision"
                )
                proc, new_base = start(project, log)
                assert new_base != base
                for _ in range(600):
                    if httpx.get(new_base + "/_portal/health").json()["ready"]:
                        break
                    time.sleep(0.1)
                else:
                    raise AssertionError("restart not ready")
                assert (project / ".slowbooks/secrets.json").read_bytes() == keys
                customers = cli(project, "api", "/api/customers")["body"]
                assert "Persisted Customer" in json.dumps(customers)
                snapshots = list(
                    (project / ".slowbooks/startup-backups").glob("*/company.db")
                )
                assert snapshots
                assert not snapshots[0].read_bytes().startswith(b"SQLite format 3")
                sys.path.insert(0, str(ROOT))
                import launcher

                env = launcher.project_environment(project / ".slowbooks", 0)
                check = subprocess.run(
                    [
                        str(launcher.PYTHON),
                        "-c",
                        "from app.services import company_file as c; import sys; "
                        "db=c.connect_raw(sys.argv[1], c.resolve_key()); "
                        'assert db.execute("select count(*) from customers").fetchone()[0] == 1; db.close()',
                        str(snapshots[0]),
                    ],
                    env=env,
                    capture_output=True,
                    text=True,
                )
                assert check.returncode == 0, check.stderr
                # Unauthenticated requests remain denied through the gateway.
                assert httpx.get(new_base + "/api/accounts").status_code == 401
                assert cli(project, "doctor")["running"]
                print(
                    "PASS: first-run UI, native API roles, PDF export, persistent keys/books, "
                    "encrypted upgrade backup, restart on a different port, desktop/mobile screenshots"
                )
            finally:
                stop(proc)
                if proc.returncode not in (0, -15):
                    print((project / "service.log").read_text(), file=sys.stderr)


if __name__ == "__main__":
    main()
