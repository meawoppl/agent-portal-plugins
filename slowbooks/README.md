# SlowBooks for Agent Portal

Run the **unmodified SlowBooks Pro 2026 browser app** beside an agent conversation.
The connector manages startup, a pinned Python environment and source checkout,
project-local storage, and authenticated API access. No Docker or desktop WebView
is needed. Linux and macOS; Python 3.12/3.13, Git and [uv](https://docs.astral.sh/uv/).
PDF rendering requires Pango (Ubuntu: `libpango-1.0-0 libpangoft2-1.0-0`; macOS:
`brew install pango`). Setup checks the PDF and SQLCipher imports.

```sh
agent-portal plugin install github:meawoppl/agent-portal-plugins//slowbooks
agent-portal plugin open slowbooks
```

On first open, the pane displays startup progress while migrations and the initial
chart of accounts are prepared. Set your company name and operator password in
SlowBooks' own setup form. Its dashboard, customers/vendors, invoices, banking,
reports, imports, settings and backup screens are the upstream application.
The upstream UI is designed for desktop widths; narrow mobile panes retain its
horizontal scrolling. Use a wider pane for bookkeeping forms.

Creating an invoice in SlowBooks **posts it to the books**; estimates are the
appropriate starting point when a user wants a quotation.

## Storage and restarts

Each project gets `.slowbooks-project.json` (a discovery marker) and a private,
self-ignored `.slowbooks/` directory. Use a separate project directory for each
set of books. No sample transactions or default password are installed.

- `companies/`: SQLite company files, including attachments stored by SlowBooks.
- `companies.json`: upstream company manifest and selected company.
- `secrets.json`: persistent session and encryption keys. **Keep this with backups**;
  losing these keys can make encrypted fields and company files unrecoverable.
- `backups/`: backups created by the SlowBooks UI.
- `startup-backups/`: consistent database snapshot plus keys/manifest before an
  upstream-revision upgrade of existing books. These are not automatically deleted.
- `startup.log`, `service.json`, `service.lock`: local diagnostics and lifecycle state.
- `api-token`: optional scoped API credential, mode 0600.

The managed upstream checkout and venv live inside the plugin, separate from
company data. Plugin updates do not replace project data or rotate keys.
Portal owns the service and injects its port on each launch; restarting the
session or changing its assigned port restarts the same project safely. A
project lock prevents competing services. If another session already owns the
project, stop that surface before opening a second one.

```sh
agent-portal plugin update slowbooks
agent-portal plugin restart slowbooks
agent-portal plugin open slowbooks
```

The pane's health endpoint is available during migrations; readiness and startup
errors are explicit in `/_portal/health`. `/health` is SlowBooks' actual readiness
endpoint. Initialization failures remain visible in the pane and `startup.log`.
After correcting a problem, restart the plugin. Bookkeeping requests are never
forwarded to a partly initialized app.

## Agent/API use

Create a scoped API token in SlowBooks **Settings → API tokens**. Start with a
read-only role for inspection, and choose a write-capable role only for work the
user requested. The connector preserves upstream role enforcement and audit
attribution. It neither bypasses authentication nor creates admin tokens.

```sh
# Run from the installed plugin directory (agent-portal plugin runtime slowbooks).
# Supply the token on stdin, not in command history or an agent-visible argument.
bin/slowbooks token --cwd /path/to/project < /private/path/token.txt
bin/slowbooks doctor --cwd /path/to/project
bin/slowbooks api --cwd /path/to/project /api/accounts
bin/slowbooks api --cwd /path/to/project /api/customers
# Explicit mutation; payload comes from a reviewed JSON file.
bin/slowbooks api --cwd /path/to/project /api/customers \
  --method POST --json-file customer.json
```

`SLOWBOOKS_API_TOKEN` overrides the stored token. Responses are JSON objects with
`status` and `body`; HTTP errors exit nonzero. For CSV/PDF downloads, use `--output`.
Payment retries can pass the same `--idempotency-key` supported by upstream.
Credentials are sent only to the project's running loopback server; redirects
are not followed. Token principals cannot manage tokens or call upstream's
admin-only backup, company, and migration endpoints. Use the authenticated UI
for those tasks. The command does not retry mutations automatically.

## Reproducibility and licensing

`upstream.json` pins SlowBooks **2.23.2**, commit
`3a571ad6978af9160e751f26a89feeab027d963c`. `uv.lock` pins dependencies derived
from that revision's requirements. Setup downloads the original checkout with
its LICENSE and attribution intact. No upstream source, templates or assets
are copied into this connector. SlowBooks is not rebranded or patched.

The connector is MIT licensed. SlowBooks is separately licensed under the
[SlowBooks Source Available License v2.0](https://github.com/VonHoltenCodes/SlowBooks-Pro-2026/blob/3a571ad6978af9160e751f26a89feeab027d963c/LICENSE),
including its restrictions on paid redistribution/hosting; installing the
connector does not grant additional rights to SlowBooks.

SlowBooks Pro 2026 — originally created by Trent Von Holten.
Copyright (c) 2026 Trent Von Holten.
[Upstream project](https://github.com/VonHoltenCodes/SlowBooks-Pro-2026).

## Development

```sh
bin/slowbooks setup
uv sync --locked
uv run --locked pytest tests -q
uv run --locked playwright install chromium
uv run --locked python tests/integration.py
agent-portal plugin validate . --json
```

The integration test uses temporary company data, exercises first-run UI and
API authentication/roles, PDF export, port-changing restart and persisted data,
and captures desktop/mobile screenshots. It never opens your real books.
