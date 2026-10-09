"""yapcad_patches against the pinned yapCAD (plugin venv, no pythonocc)."""

import unittest
import warnings

warnings.filterwarnings("ignore")

import numpy as np  # noqa: E402

import yapcad_patches  # noqa: E402

yapcad_patches.apply()

from yapcad.dsl import compile_and_run  # noqa: E402
from yapcad.mesh import mesh_view  # noqa: E402

SOURCE = """module p
command SUM(n: int = 4) -> float:
    x: float = 0.0
    for i in range(n):
        x = x + 1.0
        if i > 1:
            x = x + 10.0
    emit x
command SHADOW() -> float:
    x: float = 1.0
    for i in range(3):
        x: float = 100.0
    emit x
command STACK(n: int = 3) -> solid:
    s: solid = box(1.0, 1.0, 1.0)
    for i in range(n):
        s = union(s, translate(box(1.0, 1.0, 1.0), 0.0, 0.0, 1.0 * (i + 1)))
    emit s
@meta(material={"name": "NdFeB", "color": [0.9, 0.2, 0.2]})
command MAG() -> solid:
    emit box(10.0, 5.0, 3.0)
command CUT() -> solid:
    emit difference(MAG(), cylinder(1.0, 10.0))
command PLATE() -> solid:
    emit extrude(difference2d(rectangle(20.0, 10.0), disk(point(0.0, 0.0), 3.0)), 2.0)
command D() -> solid:
    emit extrude(difference2d(disk(point(0.0, 0.0), 10.0), rectangle(30.0, 30.0, point2d(-15.0, 0.0))), 1.0)
command HELIX(twist: float = 90.0) -> solid:
    emit helical_extrude(rectangle(4.0, 1.0, point2d(6.0, 0.0)), 20.0, twist)
"""


def run(command, **params):
    result = compile_and_run(SOURCE, command, params)
    assert result.success, result.error_message
    return result.geometry


def triangles(solid):
    return np.array(
        [[v0[:3], v1[:3], v2[:3]] for _, v0, v1, v2 in mesh_view(solid)], dtype=float
    )


def volume(tris):
    a, b, c = tris[:, 0], tris[:, 1], tris[:, 2]
    return abs(np.einsum("ij,ij->i", a, np.cross(b, c)).sum() / 6)


class PatchTests(unittest.TestCase):
    def test_applied(self):
        self.assertEqual(
            yapcad_patches.apply(),
            ["loop_reassignment", "boolean_metadata", "mesh_extrude"],
        )

    def test_block_reassignment_updates_outer_binding(self):
        self.assertEqual(run("SUM"), 24.0)
        self.assertEqual(run("SHADOW"), 1.0, "typed declarations still shadow")
        top = triangles(run("STACK"))[..., 2].max()
        self.assertAlmostEqual(top, 3.5)

    def test_booleans_keep_first_operand_metadata(self):
        meta = run("CUT")[4]
        self.assertEqual(meta["material"]["name"], "NdFeB")
        self.assertEqual(meta["material"]["color"], [0.9, 0.2, 0.2])

    def test_mesh_extrude_regions(self):
        plate = triangles(run("PLATE"))
        self.assertAlmostEqual(volume(plate), (200 - np.pi * 9) * 2, delta=1.0)
        d = triangles(run("D"))
        self.assertAlmostEqual(volume(d), np.pi * 100 / 2, delta=1.0)
        self.assertGreaterEqual(d[..., 0].min(), -1e-6, "D keeps the +x half")

    def test_helical_extrude_twists_counter_clockwise(self):
        tris = triangles(run("HELIX", twist=90.0))
        self.assertAlmostEqual(volume(tris), 80.0, delta=1.0)
        points = tris.reshape(-1, 3)
        top = points[points[:, 2] > 19.99]
        self.assertAlmostEqual(top[:, 0].mean(), 0.0, delta=0.05)
        self.assertAlmostEqual(top[:, 1].mean(), 6.0, delta=0.05)


if __name__ == "__main__":
    unittest.main()
