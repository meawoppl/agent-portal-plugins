import asyncio
import json
import shutil
import tempfile
import unittest
from pathlib import Path

from server import Bench, Workbench, discover, failed_log, inside


class ConfigTests(unittest.TestCase):
    def test_legacy_discovery(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "tesla_ctl/fpga").mkdir(parents=True)
            (root / "tesla_ctl/fpga_test").mkdir()
            (root / "tesla_ctl/fpga/pulse.v").write_text("module pulse; endmodule")
            (root / "tesla_ctl/fpga_test/pulse_tb.v").write_text(
                "module pulse_tb; endmodule"
            )
            (bench,) = discover(root)
            self.assertEqual(bench.top, "pulse_tb")
            self.assertEqual(len(bench.sources), 2)

    def test_config_rejects_escapes_and_duplicates(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            with self.assertRaises(ValueError):
                inside(root, "../secret")
            (root / "tb.sv").write_text("module tb; endmodule")
            item = {"id": "one", "top": "tb", "sources": ["tb.sv"]}
            config = root / ".verilog-workbench.json"
            config.write_text(json.dumps({"version": 1, "tests": [item, item]}))
            with self.assertRaisesRegex(ValueError, "Duplicate"):
                discover(root)
            item["sources"] = ["missing.v"]
            config.write_text(json.dumps({"version": 1, "tests": [item]}))
            with self.assertRaisesRegex(ValueError, "Missing"):
                discover(root)

    def test_legacy_error(self):
        self.assertTrue(failed_log("ERROR: uart not enabling"))
        self.assertFalse(failed_log("VCD info: dumpfile output.vcd opened"))
        self.assertFalse(failed_log('$ ["/home/error/iverilog"]\nVCD info: opened'))


@unittest.skipUnless(
    shutil.which("iverilog") and shutil.which("vvp"), "Icarus required"
)
class SimulationTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.work = Workbench(self.root)

    async def asyncTearDown(self):
        self.temp.cleanup()

    def bench(self, body, timeout=5):
        (self.root / "tb.sv").write_text(body)
        return Bench("tb", "tb", ["tb.sv"], [], [], [], timeout)

    async def test_zero_exit_error_retains_wave_and_snapshot(self):
        bench = self.bench("""`timescale 1ns/1ps
module tb;
reg a=0;
initial begin
$dumpfile("output.vcd"); $dumpvars(0,tb);
#1 a=1; #1 $display("ERROR: failed expectation"); $finish;
end
endmodule""")
        result = await self.work.run(bench)
        self.assertEqual(result["simulation_exit"], 0)
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["waves"], ["output.vcd"])
        (self.root / "tb.sv").write_text("changed")
        self.assertIn(
            "module tb", (self.work.runs / result["id"] / "sources/tb.sv").read_text()
        )
        wave = await self.work.wave(result["id"], "output.vcd")
        self.assertEqual(wave["tb.a"].tv, [(0, "0"), (1000, "1")])

    async def test_project_root_include_is_snapshotted_without_run_recursion(self):
        (self.root / "defs.vh").write_text("`define VALUE 7\n")
        bench = self.bench(
            '`include "defs.vh"\nmodule tb; initial begin assert(`VALUE == 7) else $fatal; $finish; end endmodule'
        )
        bench.includes = ["."]
        for _ in range(2):
            result = await self.work.run(bench)
            self.assertEqual(result["status"], "passed")
            files = list((self.work.runs / result["id"] / "sources").rglob("defs.vh"))
            self.assertEqual(len(files), 1)

    async def test_compile_error_timeout_and_cancel(self):
        result = await self.work.run(self.bench("module tb; INVALID endmodule"))
        self.assertEqual(result["status"], "compile-failed")
        endless = self.bench("module tb; initial forever #1; endmodule", timeout=1)
        result = await self.work.run(endless)
        self.assertEqual(result["status"], "timeout")
        endless.timeout = 10
        task = asyncio.create_task(self.work.run(endless))
        while not self.work.process:
            await asyncio.sleep(0.01)
        task.cancel()
        result = await task
        self.assertEqual(result["status"], "cancelled")
        self.assertIsNone(self.work.process)
        self.assertIsNone(self.work.active)


if __name__ == "__main__":
    unittest.main()
