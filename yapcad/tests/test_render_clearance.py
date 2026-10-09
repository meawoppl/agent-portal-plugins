import base64
import tempfile
import unittest
from pathlib import Path

import numpy as np
from PIL import Image

import clearance
import render
from server import normalized_argv


def box(lo, hi):
    x0, y0, z0 = lo
    x1, y1, z1 = hi
    v = np.array(
        [[x, y, z] for x in (x0, x1) for y in (y0, y1) for z in (z0, z1)], dtype=float
    )
    faces = [
        (0, 1, 3), (0, 3, 2), (4, 6, 7), (4, 7, 5), (0, 4, 5), (0, 5, 1),
        (2, 3, 7), (2, 7, 6), (0, 2, 6), (0, 6, 4), (1, 5, 7), (1, 7, 3),
    ]  # fmt: skip
    return v[np.array(faces)]


def part(name, tris, color, material=None, watertight=True):
    return {
        "name": name,
        "material": material,
        "color": color,
        "positions": base64.b64encode(tris.astype(np.float32).tobytes()).decode(),
        "stats": {"watertight": watertight},
    }


def view(*parts):
    pts = np.concatenate(
        [
            np.frombuffer(base64.b64decode(p["positions"]), dtype=np.float32).reshape(
                -1, 3
            )
            for p in parts
        ]
    )
    return {
        "kind": "solid",
        "parts": list(parts),
        "stats": {"bbox": [pts.min(0).tolist(), pts.max(0).tolist()]},
    }


class RenderTests(unittest.TestCase):
    def test_depth_buffer_hides_far_geometry_behind_big_near_face(self):
        # A large thin red plate in front (z up, viewed from top) and many
        # small blue boxes behind it: a painter's sort by triangle centroid
        # draws the plate's two big triangles first and the boxes over them.
        plate = box((-20, -20, 9), (20, 20, 10))
        cubes = np.concatenate(
            [
                box((x, y, -5), (x + 2, y + 2, 8.5))
                for x in range(-16, 16, 6)
                for y in range(-16, 16, 6)
            ]
        )
        v = view(
            part("plate", plate, [0.9, 0.1, 0.1]), part("cubes", cubes, [0.1, 0.1, 0.9])
        )
        with tempfile.TemporaryDirectory() as folder:
            out = render.render(v, Path(folder) / "top.png", ["top"], (300, 300))
            # Central region only: the corner triad and size label are overlays.
            px = (
                np.asarray(Image.open(out).convert("RGB"))[40:260, 40:260]
                .reshape(-1, 3)
                .astype(int)
            )
        blue = ((px[:, 2] > 120) & (px[:, 2] > px[:, 0] + 40)).sum()
        red = ((px[:, 0] > 120) & (px[:, 0] > px[:, 2] + 40)).sum()
        self.assertGreater(red, 15000)
        self.assertLess(blue, 50, "far boxes must not show through the near plate")

    def test_clip_caps_closed_parts(self):
        tris = box((-5, -5, -5), (5, 5, 5))
        cut, capped = render.clip_triangles(tris, render.parse_clip("x>0"))
        self.assertTrue(capped)
        self.assertGreaterEqual(cut[..., 0].min(), -1e-6)
        n, off = render.parse_clip("y<2")
        np.testing.assert_allclose(n, [0, -1, 0])
        self.assertEqual(off, -2)
        with self.assertRaises(ValueError):
            render.parse_clip("w>1")
        with tempfile.TemporaryDirectory() as folder:
            render.render(
                view(part("b", tris, [0.5, 0.6, 0.9])),
                Path(folder) / "c.png",
                ["iso"],
                (200, 200),
                clip="x>0",
            )

    def test_negative_view_vectors_parse(self):
        self.assertEqual(
            normalized_argv(["render", "--view", "-1,1,0.5", "--size", "10x10"]),
            ["render", "--view=-1,1,0.5", "--size", "10x10"],
        )
        self.assertEqual(
            normalized_argv(["render", "--view", "iso"]), ["render", "--view", "iso"]
        )


class ClearanceTests(unittest.TestCase):
    def test_gap_contact_interference(self):
        a = part("pl", box((0, 0, 0), (10, 10, 10)), [1, 0, 0], "payload Ti")
        for gap, status in ((1.5, "clear"), (0.0, "contact"), (-1.0, "interference")):
            b = part(
                "bus", box((10 + gap, 0, 0), (20 + gap, 10, 10)), [0, 0, 1], "bus Al"
            )
            r = clearance.clearance(view(a, b), "payload")
            self.assertEqual(r["status"], status, r)
            self.assertAlmostEqual(r["min_distance_mm"], max(gap, 0.0), places=6)
        r = clearance.clearance(
            view(a, part("bus", box((11, 0, 0), (20, 10, 10)), [0, 0, 1], "bus Al")),
            "payload",
            minimum=2.0,
        )
        self.assertEqual(r["status"], "too_close")

    def test_exclude_and_errors(self):
        a = part("pl", box((0, 0, 0), (1, 1, 1)), [1, 0, 0], "payload")
        flex = part("flex", box((1, 0, 0), (2, 1, 1)), [0, 1, 0], "flex loop")
        bus = part("bus", box((5, 0, 0), (6, 1, 1)), [0, 0, 1], "bus")
        r = clearance.clearance(view(a, flex, bus), "payload", exclude="flex")
        self.assertEqual(
            (r["status"], r["min_distance_mm"], r["excluded"]), ("clear", 4.0, ["flex"])
        )
        with self.assertRaises(ValueError):
            clearance.clearance(view(a, bus), "nothing-matches")

    def test_exact_triangle_distance_cases(self):
        t = np.array([[[0, 0, 0], [1, 0, 0], [0, 1, 0]]], dtype=float)
        above = t + [0.2, 0.2, 2.0]  # face over face
        edge = np.array(
            [[[2, -1, 0.5], [2, 2, 0.5], [3, 0, 0.5]]], dtype=float
        )  # beside an edge
        cross = np.array(
            [[[0.25, 0.25, -1], [0.25, 0.25, 1], [0.3, 0.2, 0]]], dtype=float
        )  # pierces
        self.assertAlmostEqual(clearance.triangle_distance(t, above)[0][0], 2.0)
        self.assertAlmostEqual(
            clearance.triangle_distance(t, edge)[0][0], np.hypot(1.0, 0.5)
        )
        self.assertEqual(clearance.triangle_distance(t, cross)[0][0], 0.0)


if __name__ == "__main__":
    unittest.main()
