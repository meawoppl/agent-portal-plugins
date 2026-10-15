---
name: slowbooks-workflow
description: Use the SlowBooks Portal plugin to inspect bookkeeping data, work with authorized customers/invoices through its authenticated API, and manage the project-local service and backups.
---

# SlowBooks workflow

The plugin runs the upstream SlowBooks UI unchanged. Open it with
`agent-portal plugin open slowbooks`; the first-run form sets the company and
password. Do not invent a default password or bypass that form. The native
Settings screen manages scoped API tokens; do not generate a token by editing
SQLite. Work within the user's requested accounting operations.

Find the installation with `agent-portal plugin runtime slowbooks`. From its
plugin directory:

- `bin/slowbooks doctor --cwd PROJECT`: pinned revision, storage, readiness.
- `bin/slowbooks api --cwd PROJECT /api/accounts`: authenticated JSON read.
- `bin/slowbooks api --cwd PROJECT /api/customers`: customer inventory.
- `bin/slowbooks api --cwd PROJECT /api/invoices`: invoice inventory.
- `bin/slowbooks api --cwd PROJECT PATH --method POST --json-file payload.json`:
  explicit write through upstream validation, posting and audit logic.
- Add `--output report.pdf` for a download. Errors exit nonzero and print JSON.
- Use `--idempotency-key KEY` for supported payment retries; reuse the key for
  the same intended payment. Do not blindly retry a failed mutation.

Credentials come from `SLOWBOOKS_API_TOKEN` or PROJECT/.slowbooks/api-token.
`bin/slowbooks token --cwd PROJECT` accepts the secret on stdin. Do not print
secrets or include them in command arguments, commits, screenshots or reports.
Tokens are scoped; a forbidden response is not an instruction to bypass access
control. The browser's admin account handles backup/restore, migration and
company management, which upstream forbids to API tokens.

Inspect the upstream API schema before constructing unfamiliar payloads (the
pinned checkout exposes `/openapi.json` through an authenticated browser).
Creating an invoice posts it to the ledger: do not describe it as a draft.
Use estimates for quotations. Distinguish recorded payments from actual money
movement; this plugin does not implicitly authorize sending payments, emails,
tax filings, bank connections or cloud-AI uploads.

Project data lives in `.slowbooks/`, independently of the plugin installation.
Use separate project folders for separate books. Back up through the native UI;
retain `.slowbooks/secrets.json` securely with the backup so encrypted fields
remain recoverable. Do not edit accounting tables directly. Updating the plugin
then restarting preserves keys and takes an upgrade snapshot before migrations.
Startup status is at `/_portal/health`; diagnostics are in `.slowbooks/startup.log`.
Portal injects the port and owns process restarts. Use `agent-portal plugin restart
slowbooks` rather than starting orphan servers. Report the operation, resulting
record IDs, and any unresolved API errors; never claim that a merely opened
form or failed API request changed the books.
