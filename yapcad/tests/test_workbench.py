import shutil
import tempfile
import unittest
from pathlib import Path

import numpy as np

import render
from meshutil import feature_edges
from server import Workbench, artifact, discover, inside, parse_params, validate_spec

EXAMPLE = Path(__file__).resolve().parent.parent / "examples/bracket/bracket.dsl"


def cube(size=1.0):
    corners = np.array(
        [[x, y, z] for x in (0, size) for y in (0, size) for z in (0, size)],
        dtype=float,
    )
    faces = [
        (0, 1, 3),
        (0, 3, 2),
        (4, 6, 7),
        (4, 7, 5),
        (0, 4, 5),
        (0, 5, 1),
        (2, 3, 7),
        (2, 7, 6),
        (0, 2, 6),
        (0, 6, 4),
        (1, 5, 7),
        (1, 7, 3),
    ]
    return corners[np.array(faces)]


class HelperTests(unittest.TestCase):
    def test_params_are_json_when_possible(self):
        params = parse_params(
            ["width=60", "flag=true", "size=M10", "holes=[1, 2]", 'name="x"'],
            '{"depth": 3}',
        )
        self.assertEqual(
            params,
            {
                "depth": 3,
                "width": 60,
                "flag": True,
                "size": "M10",
                "holes": [1, 2],
                "name": "x",
            },
        )
        with self.assertRaises(ValueError):
            parse_params(["width"])

    def test_discovery_and_path_safety(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "parts").mkdir()
            (root / "parts/a.dsl").write_text("module a")
            (root / "parts/a.stl").write_bytes(b"")
            (root / "pkg.ycpkg").mkdir()
            (root / "pkg.ycpkg/manifest.yaml").write_text("schema: x")
            (root / "pkg.ycpkg/inner.stl").write_bytes(b"")
            (root / ".yapcad-runs").mkdir()
            (root / ".yapcad-runs/old.dsl").write_text("")
            found = discover(root)
            self.assertEqual(
                found,
                {
                    "sources": ["parts/a.dsl"],
                    "packages": ["pkg.ycpkg"],
                    "imports": ["parts/a.stl"],
                },
            )
            with self.assertRaises(ValueError):
                inside(root, "../escape.dsl")

    def test_spec_validation(self):
        root = EXAMPLE.parent
        spec = validate_spec(
            root,
            {"file": "bracket.dsl", "command": "PLATE", "exports": ["stl"]},
            "build",
        )
        self.assertEqual(spec["relative"], "bracket.dsl")
        for body, message in (
            ({"file": "bracket.dsl", "command": "PLATE; rm"}, "command"),
            (
                {"file": "bracket.dsl", "command": "PLATE", "exports": ["obj"]},
                "exports",
            ),
            (
                {"file": "bracket.dsl", "command": "PLATE", "representation": "brep"},
                "representation",
            ),
            ({"file": "bracket.dsl", "command": "PLATE", "timeout": 1}, "timeout"),
            ({"file": "missing.dsl", "command": "PLATE"}, "not found"),
            ({"file": "bracket.dsl"}, "Import"),
        ):
            with self.subTest(body=body), self.assertRaisesRegex(ValueError, message):
                validate_spec(root, body, "import" if message == "Import" else "build")

    def test_feature_edges_of_cube(self):
        edges = feature_edges(cube())
        # 12 cube edges; face diagonals are coplanar and must not appear.
        self.assertEqual(len(edges), 12)
        self.assertEqual(
            len(feature_edges(cube()[:-1])), 13
        )  # the opened diagonal becomes a boundary

    def test_render_views(self):
        view = {
            "kind": "solid",
            "parts": [
                {
                    "name": "cube",
                    "color": [0.5, 0.6, 0.9],
                    "positions": __import__("base64")
                    .b64encode(cube(10).astype(np.float32).tobytes())
                    .decode(),
                    "stats": {"watertight": True},
                }
            ],
            "stats": {"bbox": [[0, 0, 0], [10, 10, 10]]},
        }
        with tempfile.TemporaryDirectory() as folder:
            for views in (["sheet"], ["iso", "1,2,3"]):
                path = render.render(
                    view, Path(folder) / "out.png", views, (320, 240), "cube"
                )
                from PIL import Image

                image = Image.open(path).convert("RGB")
                pixels = np.asarray(image).reshape(-1, 3)
                lit = ((pixels[:, 2] > 120) & (pixels[:, 2] > pixels[:, 0] + 20)).sum()
                self.assertGreater(lit, 2000)
            with self.assertRaises(ValueError):
                render.direction_of("diagonal")
            render.render(
                {
                    "kind": "2d",
                    "paths": [{"points": [[0, 0], [5, 0], [5, 5]], "closed": True}],
                    "stats": {},
                },
                Path(folder) / "p.png",
            )
            render.render({"kind": "value", "value": 3.5}, Path(folder) / "v.png")


class BuildTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        shutil.copy(EXAMPLE, self.root / "bracket.dsl")
        self.work = Workbench(self.root)

    async def asyncTearDown(self):
        self.temp.cleanup()

    async def build(self, kind="build", **body):
        return await self.work.execute(
            validate_spec(self.root, {"file": "bracket.dsl", **body}, kind)
        )

    async def test_solid_build_with_exports_and_snapshot(self):
        record = await self.build(
            command="PLATE", params={"width": 80}, exports=["stl", "step", "svg"]
        )
        self.assertEqual(record["status"], "ok", record)
        self.assertEqual(record["params"], {"width": 80.0})
        stats = record["stats"]
        size = np.subtract(stats["bbox"][1], stats["bbox"][0])
        np.testing.assert_allclose(size, [80, 30, 6], atol=1e-6)
        self.assertTrue(stats["watertight"])
        self.assertEqual(stats.get("bodies", 1), 1)
        self.assertAlmostEqual(
            stats["volume"], 80 * 30 * 6 - np.pi * 6 * (5**2 + 2 * 2.25**2), delta=60
        )
        self.assertEqual(set(record["exports"]), {"stl", "step"})
        self.assertTrue(any("SVG" in n for n in record["notes"]))
        directory = Path(record["path"])
        for name in (
            "preview.png",
            "view.json",
            "model.stl",
            "model.step",
            "source.dsl",
            "run.log",
        ):
            self.assertTrue((directory / name).is_file(), name)
        self.assertEqual(artifact(directory, "model.stl"), directory / "model.stl")
        with self.assertRaises(ValueError):
            artifact(directory, "spec.tmp")
        (self.root / "bracket.dsl").write_text("module changed")
        self.assertIn("command PLATE", (directory / "source.dsl").read_text())
        self.assertEqual(self.work.history()[0]["id"], record["id"])
        self.assertEqual(self.work.run_dir("latest"), directory)
        with self.assertRaises(ValueError):
            self.work.run_dir("../../etc")

    async def test_require_failure_reports_line(self):
        record = await self.build(command="PLATE", params={"width": 10})
        self.assertEqual(record["status"], "failed")
        self.assertIn("line 13", record["message"])

    async def test_unknown_parameter_and_command(self):
        record = await self.build(command="PLATE", params={"wdith": 10})
        self.assertEqual(record["status"], "failed")
        self.assertIn("wdith", record["message"])
        record = await self.build(command="NOPE")
        self.assertIn("NOPE", record["message"])

    async def test_2d_and_value_results(self):
        record = await self.build(command="PROFILE", exports=["dxf", "svg"])
        self.assertEqual((record["status"], record["result"]), ("ok", "2d"))
        self.assertEqual(set(record["exports"]), {"dxf", "svg"})
        self.assertIn("#1a1b26", (Path(record["path"]) / "model.svg").read_text())
        record = await self.build(command="MASS_G", params={"thickness": 10})
        self.assertEqual((record["status"], record["result"]), ("ok", "value"))
        self.assertAlmostEqual(record["value"], 60 * 30 * 10 * 0.0027)

    async def test_package_and_reimport(self):
        record = await self.build("package", command="PLATE", package={"name": "plate"})
        self.assertEqual(record["status"], "ok", record)
        self.assertTrue(record["package"]["valid"], record["package"])
        package = Path(record["path"]) / "package.ycpkg"
        self.assertTrue((package / "manifest.yaml").is_file())
        build = await self.build(command="PLATE", exports=["stl"])
        shutil.copy(Path(build["path"]) / "model.stl", self.root / "plate.stl")
        imported = await self.work.execute(
            validate_spec(self.root, {"file": "plate.stl"}, "import")
        )
        self.assertEqual(imported["status"], "ok", imported)
        np.testing.assert_allclose(
            imported["stats"]["bbox"], build["stats"]["bbox"], atol=1e-4
        )
        shutil.copytree(package, self.root / "plate.ycpkg")
        imported = await self.work.execute(
            validate_spec(self.root, {"file": "plate.ycpkg"}, "import")
        )
        self.assertEqual(imported["status"], "ok", imported)
        self.assertTrue(imported["package"]["valid"])

    async def test_listing(self):
        listing = await self.work.commands("bracket.dsl")
        plate = next(c for c in listing["commands"] if c["name"] == "PLATE")
        width = plate["params"][0]
        self.assertEqual(
            (width["name"], width["type"], width["default"]), ("width", "float", 60.0)
        )
        self.assertEqual(width["ui"]["widget"], "slider")
        self.assertEqual(
            [c["return_type"] for c in listing["commands"]],
            ["solid", "region2d", "float"],
        )


if __name__ == "__main__":
    unittest.main()
