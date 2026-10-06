import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from server import design_args, discover
from visilog_bridge import DesignViewer, adapt_asset, executable

COUNTER = Path(__file__).resolve().parents[1] / "examples/counter"


class Config(unittest.TestCase):
    def test_counter_bench_args(self):
        (bench,) = discover(COUNTER)
        self.assertEqual(bench.top, "counter_tb")
        args = design_args(COUNTER, bench)
        self.assertEqual(args[:2], ["-s", "counter_tb"])
        self.assertIn(str(COUNTER / "rtl/counter.v"), args)
        self.assertIn(str(COUNTER / "rtl_test/counter_tb.v"), args)
        self.assertEqual(args.index("--search") + 1, args.index(str(COUNTER)))


class Assets(unittest.TestCase):
    def test_adapter(self):
        self.assertEqual(
            adapt_asset("/", b'<script src="/viewer.js">'),
            b'<script src="/design/viewer.js">',
        )
        data = adapt_asset(
            "/viewer.js",
            b'fetch(path, { headers: { "Content-Type": "application/json" } })',
        )
        self.assertIn(b'fetch("/design" + path,', data)
        self.assertIn(b'"X-Visilog-Request": "1"', data)
        with self.assertRaises(ValueError):
            adapt_asset("/viewer.js", b"upstream changed")


@unittest.skipUnless(executable(), "Visilog binary required")
class Lifecycle(unittest.IsolatedAsyncioTestCase):
    async def test_load_step_restart_cleanup(self):
        (bench,) = discover(COUNTER)
        with tempfile.TemporaryDirectory() as directory:
            viewer = DesignViewer(Path(directory))
            try:
                result = await viewer.start(design_args(COUNTER, bench), bench.timeout)
                self.assertEqual(result["url"], "/design/")
                old = viewer.process
                async with viewer.client.post(
                    viewer.base + "/api/run", json={"kind": "step"}
                ) as response:
                    self.assertEqual(response.status, 200)
                    self.assertTrue(await response.json())
                await viewer.start(design_args(COUNTER, bench), bench.timeout)
                self.assertIsNotNone(old.returncode)
                current = viewer.process
            finally:
                await viewer.close()
            self.assertIsNotNone(current.returncode)

    async def test_bad_source_reports_log(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "broken_tb.v").write_text("module broken_tb; this is not verilog")
            (bench,) = discover(root)
            viewer = DesignViewer(root / "runs")
            with self.assertRaises(ValueError) as caught:
                await viewer.start(design_args(root, bench), 10)
            self.assertNotIn("$ ", str(caught.exception))
            self.assertFalse(viewer.running)
