"""Headless previews of run views (view.json) for agents and annotations.

Solids use a depth-sorted, back-face-culled painter's rasterizer with Pillow:
adequate for closed CAD meshes, not a photorealistic renderer. 2D results are
drawn as polylines. Both use the Portal's dark palette."""

from __future__ import annotations

import base64
import json
import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

from meshutil import feature_edges

BACKGROUND = (26, 27, 38)
TEXT = (192, 202, 245)
MUTED = (86, 95, 137)
STROKE = (122, 162, 247)
# Camera directions point from the model toward the viewer (Z up).
VIEWS = {
    "iso": (1.0, -1.0, 0.8),
    "front": (0.0, -1.0, 0.0),
    "back": (0.0, 1.0, 0.0),
    "top": (0.0, 0.0, 1.0),
    "bottom": (0.0, 0.0, -1.0),
    "right": (1.0, 0.0, 0.0),
    "left": (-1.0, 0.0, 0.0),
}
SHEET = ("iso", "front", "top", "right")


def font(size):
    for name in ("DejaVuSans.ttf", "Arial.ttf", "Helvetica.ttc"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


def parts(view):
    for part in view.get("parts", []):
        if part.get("hidden"):
            continue
        triangles = np.frombuffer(base64.b64decode(part["positions"]), dtype=np.float32)
        triangles = triangles.reshape(-1, 3, 3).astype(np.float64)
        if part.get("edges") is not None:
            edges = (
                np.frombuffer(base64.b64decode(part["edges"]), dtype=np.float32)
                .reshape(-1, 2, 3)
                .astype(np.float64)
            )
        else:
            edges = feature_edges(triangles)
        yield part, triangles, edges


def basis(direction, up_hint=None):
    forward = np.asarray(direction, dtype=float)
    forward /= np.linalg.norm(forward)
    up = np.array(
        up_hint or ((0.0, 1.0, 0.0) if abs(forward[2]) > 0.99 else (0.0, 0.0, 1.0))
    )
    right = np.cross(up, forward)
    right /= np.linalg.norm(right)
    return right, np.cross(forward, right), forward


def draw_edges(draw, ids, faces, segments, tolerance):
    """Draw feature edges where they are not behind the nearest painted face."""
    if not len(segments):
        return
    owners = np.asarray(ids, dtype=np.int64)
    # Per-face screen-space depth planes: depth = a*x + b*y + c.
    matrix = np.concatenate([faces[..., :2], np.ones(faces.shape[:2] + (1,))], axis=2)
    planes = np.einsum("nij,nj->ni", np.linalg.pinv(matrix), faces[..., 2])
    for start, end in segments:
        length = float(np.hypot(*(end[:2] - start[:2])))
        steps = int(min(max(length / 3, 2), 400))
        t = np.linspace(0, 1, steps)[:, None]
        points = start + (end - start) * t
        x = np.clip(points[:, 0].astype(int), 0, owners.shape[1] - 1)
        y = np.clip(points[:, 1].astype(int), 0, owners.shape[0] - 1)
        owner = owners[y, x]
        face_depth = np.where(
            owner >= 0,
            (
                planes[np.maximum(owner, 0)]
                * np.stack([points[:, 0], points[:, 1], np.ones(steps)], 1)
            ).sum(1),
            -np.inf,
        )
        visible = points[:, 2] >= face_depth - tolerance
        run = None
        for index in range(steps + 1):
            if index < steps and visible[index]:
                run = index if run is None else run
            elif run is not None:
                last = index - 1
                if last > run:
                    draw.line(
                        [tuple(points[run, :2]), tuple(points[last, :2])],
                        fill=(20, 22, 32),
                        width=3,
                    )
                run = None


def render_solid(view, direction, size, title=None, edges=True):
    width, height = size
    scale_factor = 2  # supersample, then downscale for anti-aliasing
    w, h = width * scale_factor, height * scale_factor
    image = Image.new("RGB", (w, h), BACKGROUND)
    draw = ImageDraw.Draw(image)
    right, up, forward = basis(direction)
    light = np.array([0.35, 0.25, 0.9])
    light = light[0] * right + light[1] * up + light[2] * forward
    light /= np.linalg.norm(light)
    batches = []
    for part, triangles, part_edges in parts(view):
        if len(triangles):
            batches.append((part, triangles, part_edges))
    if not batches:
        return image.resize(size, Image.LANCZOS)
    points = np.concatenate([t.reshape(-1, 3) for _, t, _ in batches])
    projected = np.stack([points @ right, points @ up], axis=1)
    lo, hi = projected.min(axis=0), projected.max(axis=0)
    margin = 0.08
    extent = max(float((hi - lo).max()), 1e-9)
    scale = min(
        w * (1 - 2 * margin) / max(hi[0] - lo[0], extent * 1e-3),
        h * (1 - 2 * margin) / max(hi[1] - lo[1], extent * 1e-3),
    )
    center = (lo + hi) / 2
    faces, colors, segments = [], [], []
    for part, triangles, part_edges in batches:
        normals = np.cross(
            triangles[:, 1] - triangles[:, 0], triangles[:, 2] - triangles[:, 0]
        )
        lengths = np.linalg.norm(normals, axis=1)
        valid = lengths > 0
        normals[valid] /= lengths[valid, None]
        facing = normals @ forward
        # Meshes from booleans occasionally carry flipped facets; shade both sides.
        shade = np.abs(normals @ light)
        keep = valid & (facing > -0.02) if part["stats"].get("watertight") else valid
        base = np.array(part["color"][:3])
        rgb = np.clip(
            base[None, :] * (0.28 + 0.72 * shade[:, None]) + 0.08 * shade[:, None] ** 8,
            0,
            1,
        )
        faces.append(triangles[keep])
        colors.append((rgb[keep] * 255).astype(np.uint8))
        if edges:
            segments.append(part_edges)
    faces = np.concatenate(faces)
    colors = np.concatenate(colors)

    def to_screen(world):
        out = np.empty(world.shape[:-1] + (3,))
        out[..., 0] = (world @ right - center[0]) * scale + w / 2
        out[..., 1] = h / 2 - (world @ up - center[1]) * scale
        out[..., 2] = world @ forward
        return out

    screen = to_screen(faces)
    order = np.argsort(screen[..., 2].mean(axis=1))
    ids = Image.new("I", (w, h), -1)
    id_draw = ImageDraw.Draw(ids)
    for index in order:
        polygon = [tuple(p) for p in screen[index, :, :2]]
        draw.polygon(polygon, fill=tuple(int(c) for c in colors[index]))
        id_draw.polygon(polygon, fill=int(index))
    if segments:
        draw_edges(
            draw, ids, screen, to_screen(np.concatenate(segments)), extent * 2e-3
        )
    image = image.resize(size, Image.LANCZOS)
    annotate(image, view, title, right, up)
    return image


def annotate(image, view, title, right=None, up=None):
    draw = ImageDraw.Draw(image)
    small = font(13)
    if title:
        draw.text((10, 8), title, fill=TEXT, font=font(15))
    bbox = (view.get("stats") or {}).get("bbox")
    if bbox:
        size = np.array(bbox[1]) - np.array(bbox[0])
        label = " x ".join(f"{v:.4g}" for v in size) + " mm"
        draw.text((10, image.height - 22), label, fill=MUTED, font=small)
    if right is not None:
        # Axis triad in the lower-right corner.
        origin = np.array([image.width - 40, image.height - 34])
        for axis, color, name in (
            (0, (247, 118, 142), "X"),
            (1, (158, 206, 106), "Y"),
            (2, (122, 162, 247), "Z"),
        ):
            vector = np.eye(3)[axis]
            delta = np.array([vector @ right, -(vector @ up)]) * 20
            if np.linalg.norm(delta) < 2:
                continue
            draw.line([tuple(origin), tuple(origin + delta)], fill=color, width=2)
            draw.text(tuple(origin + delta * 1.25 - 4), name, fill=color, font=small)


def render_2d(view, size, title=None):
    width, height = size
    s = 2
    image = Image.new("RGB", (width * s, height * s), BACKGROUND)
    draw = ImageDraw.Draw(image)
    points = [p for path in view.get("paths", []) for p in path["points"]]
    if points:
        array = np.array(points)
        lo, hi = array.min(axis=0), array.max(axis=0)
        span = np.maximum(hi - lo, 1e-9)
        scale = min(width * s * 0.84 / span[0], height * s * 0.84 / span[1])
        center = (lo + hi) / 2

        def screen(p):
            return (
                (p[0] - center[0]) * scale + width * s / 2,
                height * s / 2 - (p[1] - center[1]) * scale,
            )

        for path in view["paths"]:
            coords = [screen(p) for p in path["points"]]
            if len(coords) == 1:
                x, y = coords[0]
                draw.ellipse([x - 5, y - 5, x + 5, y + 5], fill=(224, 175, 104))
                continue
            if path.get("closed"):
                coords.append(coords[0])
            draw.line(coords, fill=STROKE, width=3, joint="curve")
    image = image.resize(size, Image.LANCZOS)
    annotate(image, view, title)
    return image


def render_value(view, size, title=None):
    image = Image.new("RGB", size, BACKGROUND)
    draw = ImageDraw.Draw(image)
    draw.text((16, 16), title or "", fill=MUTED, font=font(15))
    draw.text((16, 48), str(view.get("value"))[:400], fill=TEXT, font=font(22))
    return image


def render(view, output: Path, views=("sheet",), size=(960, 720), title=None):
    """Render view.json to a PNG. 'sheet' is a 2x2 iso/front/top/right layout."""
    kind = view.get("kind")
    if kind == "2d":
        image = render_2d(view, size, title)
    elif kind != "solid":
        image = render_value(view, size, title)
    elif list(views) == ["sheet"]:
        tile = (size[0] // 2, size[1] // 2)
        image = Image.new("RGB", (tile[0] * 2, tile[1] * 2), BACKGROUND)
        for index, name in enumerate(SHEET):
            label = name if index or not title else f"{title} - {name}"
            image.paste(
                render_solid(view, VIEWS[name], tile, label),
                ((index % 2) * tile[0], (index // 2) * tile[1]),
            )
        draw = ImageDraw.Draw(image)
        draw.line([(tile[0], 0), (tile[0], image.height)], fill=MUTED)
        draw.line([(0, tile[1]), (image.width, tile[1])], fill=MUTED)
    else:
        tiles = [
            render_solid(view, direction_of(v), size, f"{title} - {v}" if title else v)
            for v in views
        ]
        columns = min(len(tiles), 2)
        rows = math.ceil(len(tiles) / columns)
        image = Image.new("RGB", (size[0] * columns, size[1] * rows), BACKGROUND)
        for index, tile in enumerate(tiles):
            image.paste(
                tile, ((index % columns) * size[0], (index // columns) * size[1])
            )
    output.parent.mkdir(parents=True, exist_ok=True)
    image.save(output, optimize=True)
    return output


def direction_of(name):
    if name in VIEWS:
        return VIEWS[name]
    try:
        values = [float(v) for v in name.split(",")]
    except ValueError:
        values = []
    if len(values) != 3 or not any(values):
        raise ValueError(
            f"Unknown view {name}; use {', '.join(VIEWS)}, sheet, or x,y,z"
        )
    return tuple(values)


def render_file(
    view_path: Path, output: Path, views=("sheet",), size=(960, 720), title=None
):
    return render(json.loads(view_path.read_text()), output, views, size, title)
