# Engineering Presentations

Author repository-backed Marp decks and review completed builds with a reloadable
viewer. The runtime uses Python 3.10+ and browser JavaScript; it does not install
Marp or change your deck's build toolchain.

## Publish and view

Keep your existing pinned build command. After it succeeds, publish its complete
site (HTML, relative figures/styles, and optionally PDF) to a **separate** folder:

```sh
PRESENTATION_PLUGIN="$HOME/agent-portal-plugins/engineering-presentations"
npm run build && python3 "$PRESENTATION_PLUGIN/scripts/viewer.py" publish \
  --site build/site --entry deck/index.html \
  --slides build/slides.json --output build/review
agent-portal plugin open engineering-presentations
```

Run these commands from the presentation repository. Add the publish invocation
to its supported build script so each successful rebuild updates the viewer.
The standard Portal surface reads `build/review` in that repository and uses the
port assigned by Portal. Do not replace an unrelated session's forward.

Without Portal:

```sh
python3 "$PRESENTATION_PLUGIN/scripts/viewer.py" serve \
  --directory build/review --port 48765
```

Open the server root `/`, **not** an old `/deck/` static-server URL. Existing plain
Marp servers must be replaced with this viewer; updating the plugin alone does
not change another repository's build command or its running server.

`slides.json` has `{"title":"My deck","slides":["intro","design","next"]}`.
IDs must be unique, in rendered section order, and match the rendered slide
count. Preserve them through edits. Omit `--slides` if unavailable: the viewer
falls back to a page number, clamped to the new slide count.

## Reload and freshness

**Reload slides** fetches the latest completed build and restores the same stable
slide ID. If that ID was deleted, it falls back to a valid page. The outer URL
stores the current ID and page, so a normal browser refresh also loads the newest
build and preserves position. If an iframe has keyboard focus, return focus to
the viewer controls to activate the reload button.

The toolbar displays the loaded revision and checks for updates every five
seconds while visible. Expand **Build revisions** for full loaded/latest hashes.
Updates are announced but never automatically applied: active annotations,
presenter windows, and playback are not interrupted. Explicit reload replaces
the embedded viewer and resets its transient presenter/media state.

A failed reload keeps the previous slides visible and reports the error. Missing
figures or styles fail visibly instead of showing an incomplete new revision.
There is no annotation capture or delivery service in this viewer; supplied
review systems must record their own slide ID and revision.

## Publication contract

- Invoke publication only after a successful build, never while another process
  is rebuilding the input directory. Use a fresh staging directory in build
  systems that allow concurrent builds.
- Files are copied and validated in a staging directory. Only then is an immutable
  revision installed and `latest.json` atomically replaced. Failed publication
  leaves the previous pointer untouched. Already-open revisions remain readable.
- All local assets must be bundled and use relative URLs that stay within the
  site. Absolute/remote embedded resources and symlinks are rejected; ordinary
  outgoing hyperlinks are allowed. The viewer is for trusted authored HTML.
- Every revision URL contains a SHA-256 hash of its metadata, paths, and file
  contents. HTML and its relative assets share that revision prefix. The viewer
  shell and latest pointer use `Cache-Control: no-store`; revision resources use
  immutable caching. No timestamp-based `304` responses are used.
- Do not edit published revisions or delete them while readers use them. Old
  builds are retained intentionally; prune only after review sessions close.
- PDFs included in the site are available below `/revisions/<hash>/...`; this
  viewer navigates Marp HTML, not browser-native PDF documents.

## Verification

```sh
python3 -m unittest discover -s engineering-presentations/tests
uv run --with playwright==1.56.0 python engineering-presentations/tests/browser.py
```

The browser test requires `uv run --with playwright==1.56.0 playwright install chromium`.
It exercises real publication and HTTP serving, checks changed text, SVG and CSS,
slide insertion/deletion, ordinary refresh, failed publication/reload, and mobile
controls. Runtime use has no Python package dependencies.
