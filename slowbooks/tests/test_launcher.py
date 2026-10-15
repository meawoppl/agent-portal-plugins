import base64
import json
from pathlib import Path
import stat
import sys
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import launcher


def test_project_secrets_persist_and_ignore_other_project_environment(
    tmp_path, monkeypatch
):
    data = launcher.initialize(tmp_path)
    monkeypatch.setenv("DATABASE_URL", "postgresql://other-project")
    monkeypatch.setenv("SLOWBOOKS_COMPANY_FILE_KEY", "wrong-key")
    monkeypatch.setenv("PAYROLL_ENCRYPTION_SECRET", "wrong-secret")
    first = launcher.project_environment(data, 10001)
    second = launcher.project_environment(data, 10002)
    for name in launcher.SECRET_NAMES:
        assert first[name] == second[name]
        assert first[name] != "wrong-secret"
    assert len(base64.urlsafe_b64decode(first["SETTINGS_ENCRYPTION_KEY"])) == 32
    assert "SLOWBOOKS_COMPANY_FILE_KEY" not in first
    assert first["DATABASE_URL"] == "sqlite:///" + str(data / "companies/company.db")
    assert stat.S_IMODE((data / "secrets.json").stat().st_mode) == 0o600
    assert stat.S_IMODE(data.stat().st_mode) == 0o700
    assert (data / ".gitignore").read_text() == "*\n"


def test_missing_keys_never_silently_replace_keys_for_existing_books(tmp_path):
    data = launcher.initialize(tmp_path)
    (data / "companies/existing.db").write_bytes(b"existing")
    with pytest.raises(ValueError, match="secrets.json is missing"):
        launcher.project_environment(data, 10001)
    assert not (data / "secrets.json").exists()


def test_only_one_service_and_stale_state_is_rejected(tmp_path):
    data = launcher.initialize(tmp_path)
    launcher.atomic_write(data / "service.json", '{"port": 10001}')
    with launcher.lock_file(data / "service.lock"):
        with pytest.raises(ValueError, match="already running"):
            launcher.lock_file(data / "service.lock")
        assert launcher.running_state(data)["port"] == 10001
    with pytest.raises(ValueError, match="stopped"):
        launcher.running_state(data)


@pytest.mark.parametrize(
    "filename", ["../outside.db", "/tmp/outside.db", ".hidden.db", "not-a-db"]
)
def test_manifest_cannot_escape_company_directory(tmp_path, filename):
    data = launcher.initialize(tmp_path)
    launcher.atomic_write(
        data / "companies.json", json.dumps({"last_opened": filename})
    )
    with pytest.raises(ValueError, match="Invalid active company"):
        launcher.project_environment(data, 10001)


@pytest.mark.parametrize(
    "url",
    [
        "https://example.com/api/accounts",
        "//example.com/api/a",
        "/not-api",
        "/api/a#fragment",
    ],
)
def test_api_credentials_cannot_be_sent_to_other_urls(url):
    with pytest.raises(ValueError, match="API path"):
        launcher.api(SimpleNamespace(path=url))


def test_redirect_does_not_forward_authorization():
    assert (
        launcher.NoRedirect().redirect_request(
            None, None, 302, "", {}, "https://example.com"
        )
        is None
    )


def test_gateway_preserves_auth_cookies_and_security_headers():
    from surface import clean_headers
    from multidict import CIMultiDict

    headers = CIMultiDict(
        [
            ("Set-Cookie", "one=1"),
            ("Set-Cookie", "two=2"),
            ("Content-Security-Policy", "frame-ancestors 'none'"),
            ("Connection", "x-private"),
            ("X-Private", "drop"),
        ]
    )
    cleaned = clean_headers(headers)
    assert cleaned.getall("Set-Cookie") == ["one=1", "two=2"]
    assert cleaned["Content-Security-Policy"] == "frame-ancestors 'none'"
    assert "X-Private" not in cleaned and "Connection" not in cleaned
