import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.request import Request, urlopen
from urllib.error import HTTPError
from http.server import ThreadingHTTPServer

SPEC = importlib.util.spec_from_file_location('viewer', Path(__file__).parents[1] / 'scripts/viewer.py')
viewer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(viewer)


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.site = self.root / 'site'
        (self.site / 'deck').mkdir(parents=True)
        (self.site / 'figures').mkdir()
        self.html = self.site / 'deck/index.html'
        self.html.write_text('<section><img src="../figures/a.svg"></section>')
        (self.site / 'figures/a.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg"/>')
        self.slides = self.root / 'slides.json'
        self.slides.write_text(json.dumps({'title': 'Demo', 'slides': ['one']}))
        self.output = self.root / 'review'

    def publish(self):
        return viewer.publish(self.site, self.output, slides=self.slides)

    def test_changed_asset_changes_revision_and_preserves_old_build(self):
        first = self.publish()
        self.assertEqual(first, self.publish())
        (self.site / 'figures/a.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg">new</svg>')
        second = self.publish()
        self.assertNotEqual(first['revision'], second['revision'])
        old = self.output / 'revisions' / first['revision'] / 'figures/a.svg'
        self.assertNotIn('new', old.read_text())
        self.assertEqual(json.loads((self.output / 'latest.json').read_text()), second)

    def test_invalid_build_does_not_replace_pointer(self):
        self.publish()
        before = (self.output / 'latest.json').read_bytes()
        (self.site / 'figures/a.svg').unlink()
        with self.assertRaisesRegex(ValueError, 'missing'):
            self.publish()
        self.assertEqual(before, (self.output / 'latest.json').read_bytes())
        self.assertEqual([], list(self.output.glob('.publish-*')))

    def test_count_duplicate_ids_escape_and_symlinks_rejected(self):
        for ids in [['one', 'two'], ['one', 'one']]:
            self.slides.write_text(json.dumps({'slides': ids}))
            with self.assertRaises(ValueError):
                self.publish()
        self.slides.write_text(json.dumps({'slides': ['one']}))
        self.html.write_text('<section><img src="/figures/a.svg"></section>')
        with self.assertRaises(ValueError):
            self.publish()
        self.html.write_text('<section></section>')
        (self.site / 'escape').symlink_to(self.slides)
        with self.assertRaisesRegex(ValueError, 'Symlinks'):
            self.publish()

    def test_concurrent_source_change_does_not_publish(self):
        first = self.publish()
        copy = viewer.shutil.copytree
        def changing_copy(source, target, *args, **kwargs):
            result = copy(source, target, *args, **kwargs)
            if Path(source) == self.site:
                self.html.write_text('<section>changed mid-publication</section>')
            return result
        with patch.object(viewer.shutil, 'copytree', changing_copy):
            with self.assertRaisesRegex(ValueError, 'changed during'):
                self.publish()
        self.assertEqual(json.loads((self.output / 'latest.json').read_text()), first)

    def test_fallback_pages_and_http_cache_contract(self):
        result = viewer.publish(self.site, self.output)
        self.assertEqual(result['slides'], [None])
        server = ThreadingHTTPServer(('127.0.0.1', 0), viewer.handler_for(self.output))
        self.addCleanup(server.server_close)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.shutdown)
        base = f'http://127.0.0.1:{server.server_port}'
        for route in ['/', '/latest.json', '/viewer.js', '/healthz']:
            with urlopen(Request(base + route, headers={'If-Modified-Since': 'Wed, 01 Jan 2099 00:00:00 GMT'})) as response:
                self.assertEqual(response.status, 200)
                self.assertEqual(response.headers['Cache-Control'], 'no-store')
        with urlopen(base + '/revisions/' + result['revision'] + '/deck/index.html') as response:
            self.assertIn('immutable', response.headers['Cache-Control'])
        for route in ['/../slides.json', '/latest.json/../../slides.json', '/revisions/' + result['revision'] + '/../latest.json']:
            with self.assertRaises(HTTPError):
                urlopen(base + route)


if __name__ == '__main__':
    unittest.main()
