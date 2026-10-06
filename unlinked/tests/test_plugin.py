import json
import sys
import tempfile
import unittest
from pathlib import Path

from aiohttp.test_utils import AioHTTPTestCase

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from server import HEADER, Workbench, discover, executable, make_app, system_names

EXAMPLES = Path(__file__).resolve().parents[1] / "examples"


class Discovery(unittest.TestCase):
    def test_examples(self):
        models, scripts = discover(EXAMPLES)
        self.assertIn("cruise_control_pi.mdl", models)
        self.assertIn("nested.mdl", models)
        self.assertEqual(scripts, ["mass_spring_damper_params.m"])

    def test_system_paths(self):
        self.assertEqual(system_names("nested"), [])
        self.assertEqual(system_names("nested/Outer//Slash/Inner"), ["Outer/Slash", "Inner"])
        self.assertEqual(system_names("m/A//B"), ["A/B"])

    def test_sim_args_validation(self):
        work = Workbench(EXAMPLES, runs=Path(tempfile.mkdtemp()))
        args, settings = work.sim_args({"stop": 2, "step": 0.5, "solver": "rk45", "vars": ["k=2*pi"]})
        self.assertEqual(settings["solver"], "rk45")
        self.assertIn("--var", args)
        for bad in (
            {"stop": 1},
            {"stop": 1, "step": 0},
            {"stop": 0, "step": 0.1},
            {"stop": 1, "step": 0.1, "solver": "ode45"},
            {"stop": 1, "step": 0.1, "vars": ["1bad=2"]},
            {"stop": 1, "step": 0.1, "vars": ["k"]},
        ):
            with self.assertRaises(ValueError):
                work.sim_args(bad)


@unittest.skipUnless(executable(), "unlinked binary required")
class Api(AioHTTPTestCase):
    async def get_application(self):
        self.directory = tempfile.TemporaryDirectory()
        return make_app(Workbench(EXAMPLES, runs=Path(self.directory.name)))

    async def asyncTearDown(self):
        await super().asyncTearDown()
        self.directory.cleanup()

    async def test_state_info_render(self):
        async with self.client.get("/api/state") as response:
            state = await response.json()
        self.assertTrue(state["ok"])
        self.assertIn("nested.mdl", state["models"])
        async with self.client.get("/api/info", params={"file": "nested.mdl"}) as response:
            info = await response.json()
        paths = [s["path"] for s in info["systems"]]
        self.assertEqual(paths, ["nested", "nested/Outer//Slash", "nested/Outer//Slash/Inner"])
        async with self.client.get(
            "/api/render", params={"file": "nested.mdl", "system": paths[2]}
        ) as response:
            self.assertEqual(response.status, 200)
            self.assertEqual(response.content_type, "image/svg+xml")
            self.assertIn(b"<svg", await response.read())
        async with self.client.get(
            "/api/render", params={"file": "nested.mdl", "system": "nested/Missing"}
        ) as response:
            self.assertEqual(response.status, 400)
            self.assertIn("Missing", (await response.json())["error"])

    async def test_rejects_escapes_and_missing_header(self):
        async with self.client.get("/api/info", params={"file": "../server.py"}) as response:
            self.assertEqual(response.status, 400)
        async with self.client.post("/api/sim", json={}) as response:
            self.assertEqual(response.status, 403)

    async def test_simulate_and_failure_log(self):
        headers = {HEADER: "1"}
        body = {"file": "cruise_control_pi.mdl", "stop": 1, "step": 0.1, "solver": "rk4"}
        async with self.client.post("/api/sim", json=body, headers=headers) as response:
            record = await response.json()
        self.assertEqual(record["status"], "completed", record)
        self.assertEqual(record["samples"], 11)
        async with self.client.get(f"/api/trace/{record['id']}") as response:
            trace = (await response.json())["trace"]
        self.assertEqual(len(trace["time"]), 11)
        body = {"file": "mass_spring_damper_pid.mdl", "stop": 0.1, "step": 0.05, "solver": "rk4"}
        async with self.client.post("/api/sim", json=body, headers=headers) as response:
            failed = await response.json()
        self.assertEqual(failed["status"], "failed")
        self.assertIn("Kp", failed["message"])
        async with self.client.get(f"/api/log/{failed['id']}") as response:
            self.assertIn("undefined variable", await response.text())
        body["vars"] = ["m=1", "b=10", "k=20", "Kp=350", "Ki=300", "Kd=50", "Tf=0.01"]
        async with self.client.post("/api/sim", json=body, headers=headers) as response:
            self.assertEqual((await response.json())["status"], "completed")
        async with self.client.get("/api/state") as response:
            runs = (await response.json())["runs"]
        self.assertEqual(len(runs), 3)
        self.assertTrue(all(json.dumps(r["settings"]) for r in runs))
