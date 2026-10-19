#!/usr/bin/env python3
"""Publish completed Marp sites as immutable revisions and serve the review viewer."""
import argparse
import hashlib
from html.parser import HTMLParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import mimetypes
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tempfile
from urllib.parse import unquote, urlsplit

VIEWER = Path(__file__).resolve().parents[1] / 'viewer'


class Document(HTMLParser):
    def __init__(self):
        super().__init__()
        self.sections = 0
        self.assets = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == 'section':
            self.sections += 1
        if tag == 'base' or attrs.get('srcset'):
            raise ValueError('Base URLs and srcset are unsupported; bundle assets with relative src URLs')
        if tag in ('image', 'use'):
            self.assets.extend(attrs[key] for key in ('href', 'xlink:href') if attrs.get(key))
        if tag == 'object' and attrs.get('data'):
            self.assets.append(attrs['data'])
        for key in ('src', 'poster'):
            if attrs.get(key):
                self.assets.append(attrs[key])
        if tag == 'link' and attrs.get('href'):
            self.assets.append(attrs['href'])


def asset_path(source, reference, root):
    parts = urlsplit(reference)
    if not reference or reference.startswith(('#', 'data:')):
        return
    if parts.scheme or parts.netloc or parts.path.startswith('/'):
        raise ValueError(f'{source}: asset must be relative and bundled: {reference}')
    target = (source.parent / unquote(parts.path)).resolve()
    if not target.is_relative_to(root) or not target.is_file():
        raise ValueError(f'{source}: missing or escaping asset: {reference}')


def validate_site(root, entry, ids):
    document = Document()
    document.feed((root / entry).read_text())
    if not document.sections:
        raise ValueError('Entry must contain rendered Marp <section> slides')
    if ids and len(ids) != document.sections:
        raise ValueError('Slide ID count does not match rendered sections')
    for file in root.rglob('*'):
        if not file.is_file():
            continue
        if file.suffix in ('.html', '.css', '.svg'):
            text = file.read_text()
            references = re.findall(r'url\(\s*[\'"]?([^\)\'"\s]+)', text)
            references += re.findall(r'@import\s+[\'"]([^\'"]+)', text)
            if file.suffix in ('.html', '.svg'):
                parser = Document()
                parser.feed(text)
                references += parser.assets
            for reference in references:
                asset_path(file, reference, root)
    return ids or [None] * document.sections


def snapshot(root):
    files = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError(f'Symlinks are not supported in published sites: {path}')
        if path.is_file():
            files[path.relative_to(root).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return files


def publish(site, output, entry='deck/index.html', slides=None, title='Presentation'):
    site, output = Path(site).resolve(), Path(output).resolve()
    entry_path = PurePosixPath(entry)
    if entry_path.is_absolute() or '..' in entry_path.parts or not (site / entry).is_file():
        raise ValueError('Entry must be a file inside the completed site')
    if output == site or output.is_relative_to(site):
        raise ValueError('Output must be separate from the input site')
    ids = []
    if slides:
        data = json.loads(Path(slides).read_text())
        ids = data['slides']
        title = data.get('title', title)
        if not isinstance(ids, list) or not all(isinstance(i, str) and i for i in ids):
            raise ValueError('slides must be an array of nonempty stable ID strings')
        if len(set(ids)) != len(ids):
            raise ValueError('Duplicate slide IDs')
    output.mkdir(parents=True, exist_ok=True)
    revisions = output / 'revisions'
    revisions.mkdir(exist_ok=True)
    # Stage first; the served pointer is replaced only after every file validates.
    with tempfile.TemporaryDirectory(prefix='.publish-', dir=output) as temporary:
        stage = Path(temporary) / 'site'
        before = snapshot(site)
        shutil.copytree(site, stage)
        if snapshot(stage) != before or snapshot(site) != before:
            raise ValueError('Site changed during publication; rebuild and retry')
        ids = validate_site(stage, entry, ids)
        metadata = {'schema': 1, 'title': title, 'entry': entry, 'slides': ids}
        digest = hashlib.sha256(json.dumps(metadata, sort_keys=True).encode())
        for path in sorted(stage.rglob('*')):
            if path.is_file():
                digest.update(path.relative_to(stage).as_posix().encode() + b'\0')
                digest.update(hashlib.sha256(path.read_bytes()).digest())
        revision = digest.hexdigest()
        metadata['revision'] = revision
        destination = revisions / revision
        if not destination.exists():
            try:
                stage.rename(destination)
            except OSError:
                if not destination.is_dir():
                    raise
        manifest = Path(temporary) / 'latest.json'
        manifest.write_text(json.dumps(metadata, ensure_ascii=False) + '\n')
        os.replace(manifest, output / 'latest.json')
    return metadata


def handler_for(directory):
    directory = Path(directory).resolve()

    class Handler(BaseHTTPRequestHandler):
        def do_HEAD(self):
            self.do_GET()

        def do_GET(self):
            route = unquote(urlsplit(self.path).path)
            immutable = False
            if route in ('/', '/index.html'):
                path = VIEWER / 'index.html'
            elif route in ('/viewer.js', '/viewer.css'):
                path = VIEWER / route[1:]
            elif route in ('/latest.json', '/healthz'):
                path = directory / 'latest.json'
            elif re.match(r'^/revisions/[a-f0-9]{64}/', route):
                path = (directory / route[1:]).resolve()
                immutable = True
                if not path.is_relative_to(directory / 'revisions') or '..' in PurePosixPath(route).parts:
                    self.send_error(404)
                    return
            else:
                self.send_error(404)
                return
            try:
                body = path.read_bytes()
            except OSError:
                self.send_error(404, 'No completed build available')
                return
            self.send_response(200)
            self.send_header('Content-Type', mimetypes.guess_type(path)[0] or 'application/octet-stream')
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Cache-Control', 'public, max-age=31536000, immutable' if immutable else 'no-store')
            self.send_header('X-Content-Type-Options', 'nosniff')
            self.end_headers()
            if self.command != 'HEAD':
                try:
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    pass
    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    pub = sub.add_parser('publish', help='Publish a completed, self-contained site')
    pub.add_argument('--site', required=True, type=Path)
    pub.add_argument('--output', type=Path, default=Path('build/review'))
    pub.add_argument('--entry', default='deck/index.html')
    pub.add_argument('--slides', type=Path, help='JSON {title, slides: [stable IDs]}')
    pub.add_argument('--title', default='Presentation')
    serve = sub.add_parser('serve', help='Serve published revisions and reload controls')
    serve.add_argument('--cwd', type=Path, default=Path.cwd())
    serve.add_argument('--directory', type=Path, default=Path('build/review'))
    serve.add_argument('--host', default='127.0.0.1')
    serve.add_argument('--port', type=int, default=48765)
    args = parser.parse_args()
    try:
        if args.command == 'publish':
            print(json.dumps(publish(args.site, args.output, args.entry, args.slides, args.title)))
        else:
            directory = args.cwd / args.directory
            if not (directory / 'latest.json').is_file():
                raise ValueError('Publish a completed build before starting the viewer')
            server = ThreadingHTTPServer((args.host, args.port), handler_for(directory))
            print(f'Presentation viewer: http://{args.host}:{server.server_port}/', flush=True)
            try:
                server.serve_forever()
            except KeyboardInterrupt:
                pass
            finally:
                server.server_close()
    except (ValueError, OSError, KeyError, json.JSONDecodeError) as error:
        parser.exit(1, f'Presentation viewer: {error}\n')


if __name__ == '__main__':
    main()
