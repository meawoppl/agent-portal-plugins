"""Consistent, encrypted-aware snapshot before changing the upstream revision."""

from contextlib import closing
from datetime import datetime, timezone
import os
from pathlib import Path
import shutil

from app.services import company_file

root = Path(os.environ["SLOWBOOKS_DATA_DIR"])
revision = os.environ["PORTAL_SLOWBOOKS_REVISION"]
path = company_file.active_path()
marker = path.with_suffix(".revision") if path else root / "runtime-revision"
if (
    path
    and path.exists()
    and path.stat().st_size
    and (not marker.exists() or marker.read_text() != revision)
):
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    destination = root / "startup-backups" / stamp
    destination.mkdir(parents=True, mode=0o700)
    key = company_file.resolve_key() if company_file.is_encrypted_file(path) else None
    with closing(company_file.connect_raw(path, key)) as source:
        with closing(company_file.connect_raw(destination / path.name, key)) as target:
            source.backup(target)
    shutil.copy2(root / "secrets.json", destination / "secrets.json")
    shutil.copy2(root / "companies.json", destination / "companies.json")
