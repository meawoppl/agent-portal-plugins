"""Headless previews of run views (view.json) for agents and annotations.

Solids use a per-pixel depth buffer (numpy batches for small triangles),
depth-tested feature edges and optional capped section clipping; flat
shaded, not photorealistic. 2D results are
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


def parse_clip(spec):
    """Section spec -> (unit normal, offset): keep points with n.p >= offset.

    Accepts ``x>40``, ``y<0``, ``z>=1.5`` or ``nx,ny,nz>offset``."""
    if not spec:
        return None
    import re

    m = re.fullmatch(
        r"\s*([xyz]|-?[\d.]+,-?[\d.]+,-?[\d.]+)\s*(>=|<=|>|<)\s*(-?[\d.]+)\s*", spec
    )
    if not m:
        raise ValueError(f"Bad clip {spec!r}; use e.g. x>0, y<12.5 or 1,1,0>3")
    axis, op, value = m.groups()
    n = (
        np.eye(3)["xyz".index(axis)]
        if axis in "xyz"
        else np.array([float(v) for v in axis.split(",")])
    )
    length = np.linalg.norm(n)
    if length == 0:
        raise ValueError("clip normal must be non-zero")
    n, offset = n / length, float(value) / length
    return (n, offset) if op.startswith(">") else (-n, -offset)


def clip_triangles(triangles, clip):
    """Cut a triangle soup by the clip half-space, capping the cut when the
    part is a closed manifold. Returns (triangles, capped)."""
    n, offset = clip
    side = triangles @ n
    if (side >= offset - 1e-9).all():
        return triangles, False
    if (side < offset).all():
        return triangles[:0], False
    try:
        from manifold3d import Manifold, Mesh

        points = triangles.reshape(-1, 3)
        unique, index = np.unique(np.round(points, 6), axis=0, return_inverse=True)
        mesh = Mesh(
            vert_properties=np.ascontiguousarray(unique, dtype=np.float32),
            tri_verts=np.ascontiguousarray(index.reshape(-1, 3), dtype=np.uint32),
        )
        solid = Manifold(mesh)
        if solid.status().name == "NoError" and not solid.is_empty():
            cut = solid.trim_by_plane(tuple(n), offset).to_mesh()
            v = np.asarray(cut.vert_properties, dtype=np.float64)[:, :3]
            return v[np.asarray(cut.tri_verts, dtype=np.int64)], True
    except Exception:  # noqa: BLE001 - fall back to an open cut
        pass
    return triangles[side.mean(axis=1) >= offset], False


def clip_segments(segments, clip):
    """Clip line segments (n, 2, 3) to the kept half-space."""
    if clip is None or not len(segments):
        return segments
    n, offset = clip
    d = segments @ n - offset
    keep = (d >= -1e-9).all(axis=1)
    out = [segments[keep]]
    cross = (d[:, 0] >= 0) != (d[:, 1] >= 0)
    for (a, b), (da, db) in zip(segments[cross], d[cross], strict=True):
        p = a + (b - a) * (da / (da - db))
        out.append(np.array([[a, p] if da >= 0 else [p, b]]))
    return np.concatenate(out) if out else segments[:0]


def rasterize(screen, colors, width, height, background):
    """Per-pixel depth-buffered fill of triangles (screen x, y, depth; larger
    depth is nearer). Small triangles are rasterized in numpy batches."""
    image = np.empty((height, width, 3), dtype=np.uint8)
    image[:] = background
    depth = np.full(height * width, -np.inf)
    flat = image.reshape(-1, 3)
    xs, ys = screen[..., 0], screen[..., 1]
    x0 = np.floor(xs.min(axis=1)).astype(np.int64).clip(0, width - 1)
    x1 = np.ceil(xs.max(axis=1)).astype(np.int64).clip(0, width - 1)
    y0 = np.floor(ys.min(axis=1)).astype(np.int64).clip(0, height - 1)
    y1 = np.ceil(ys.max(axis=1)).astype(np.int64).clip(0, height - 1)
    a, b, c = screen[:, 0], screen[:, 1], screen[:, 2]
    den = (b[:, 1] - c[:, 1]) * (a[:, 0] - c[:, 0]) + (c[:, 0] - b[:, 0]) * (
        a[:, 1] - c[:, 1]
    )
    onscreen = (
        (xs.max(axis=1) >= 0)
        & (xs.min(axis=1) < width)
        & (ys.max(axis=1) >= 0)
        & (ys.min(axis=1) < height)
    )
    live = onscreen & (np.abs(den) > 1e-12)

    def resolve(pixels, z, tri):
        if not len(pixels):
            return
        order = np.lexsort((z, pixels))
        pixels, z, tri = pixels[order], z[order], tri[order]
        last = np.r_[pixels[1:] != pixels[:-1], True]
        pixels, z, tri = pixels[last], z[last], tri[last]
        nearer = z > depth[pixels]
        depth[pixels[nearer]] = z[nearer]
        flat[pixels[nearer]] = colors[tri[nearer]]

    def fragments(idx, px, py):
        ai, bi, ci, di = a[idx], b[idx], c[idx], den[idx]
        cx, cy = px + 0.5, py + 0.5
        w0 = (
            (bi[:, 1:2] - ci[:, 1:2]) * (cx - ci[:, 0:1])
            + (ci[:, 0:1] - bi[:, 0:1]) * (cy - ci[:, 1:2])
        ) / di[:, None]
        w1 = (
            (ci[:, 1:2] - ai[:, 1:2]) * (cx - ci[:, 0:1])
            + (ai[:, 0:1] - ci[:, 0:1]) * (cy - ci[:, 1:2])
        ) / di[:, None]
        w2 = 1.0 - w0 - w1
        inside = (w0 >= -1e-6) & (w1 >= -1e-6) & (w2 >= -1e-6)
        z = w0 * ai[:, 2:3] + w1 * bi[:, 2:3] + w2 * ci[:, 2:3]
        return inside, z

    box = 8
    small = live & (x1 - x0 < box) & (y1 - y0 < box)
    gx, gy = np.meshgrid(np.arange(box), np.arange(box))
    gx, gy = gx.ravel()[None, :], gy.ravel()[None, :]
    indices = np.flatnonzero(small)
    for start in range(0, len(indices), 8192):
        idx = indices[start : start + 8192]
        px, py = x0[idx, None] + gx, y0[idx, None] + gy
        inside, z = fragments(idx, px, py)
        inside &= (px <= x1[idx, None]) & (py <= y1[idx, None])
        tri = np.broadcast_to(idx[:, None], inside.shape)
        resolve((py * width + px)[inside], z[inside], tri[inside])
    for t in np.flatnonzero(live & ~small):
        px, py = np.meshgrid(np.arange(x0[t], x1[t] + 1), np.arange(y0[t], y1[t] + 1))
        px, py = px.ravel()[None, :], py.ravel()[None, :]
        inside, z = fragments(np.array([t]), px, py)
        inside = inside[0]
        resolve(
            (py[0] * width + px[0])[inside], z[0][inside], np.full(int(inside.sum()), t)
        )
    return image, depth.reshape(height, width)


def draw_edges(draw, depth, segments, tolerance, colour=(20, 22, 32), width=3):
    """Draw feature edges where they are not behind the depth buffer."""
    h, w = depth.shape
    for start, end in segments:
        length = float(np.hypot(*(end[:2] - start[:2])))
        steps = int(min(max(length / 2, 2), 600))
        t = np.linspace(0, 1, steps)[:, None]
        points = start + (end - start) * t
        x = points[:, 0].astype(int)
        y = points[:, 1].astype(int)
        onscreen = (x >= 0) & (x < w) & (y >= 0) & (y < h)
        visible = np.zeros(steps, dtype=bool)
        visible[onscreen] = (
            points[onscreen, 2] >= depth[y[onscreen], x[onscreen]] - tolerance
        )
        run = None
        for index in range(steps + 1):
            if index < steps and visible[index]:
                run = index if run is None else run
            elif run is not None:
                if index - 1 > run:
                    draw.line(
                        [tuple(points[run, :2]), tuple(points[index - 1, :2])],
                        fill=colour,
                        width=width,
                    )
                run = None


def render_solid(view, direction, size, title=None, edges=True, clip=None):
    width, height = size
    scale_factor = 2  # supersample, then downscale for anti-aliasing
    w, h = width * scale_factor, height * scale_factor
    right, up, forward = basis(direction)
    light = np.array([0.35, 0.25, 0.9])
    light = light[0] * right + light[1] * up + light[2] * forward
    light /= np.linalg.norm(light)
    clip = parse_clip(clip) if isinstance(clip, str) else clip
    batches = []
    for part, triangles, part_edges in parts(view):
        capped = False
        if clip is not None:
            triangles, capped = clip_triangles(triangles, clip)
            part_edges = (
                feature_edges(triangles) if capped else clip_segments(part_edges, clip)
            )
        if len(triangles):
            batches.append((part, triangles, part_edges, capped))
    if not batches:
        image = Image.new("RGB", size, BACKGROUND)
        annotate(image, view, title, right, up)
        return image
    points = np.concatenate([t.reshape(-1, 3) for _, t, _, _ in batches])
    projected = np.stack([points @ right, points @ up], axis=1)
    lo, hi = projected.min(axis=0), projected.max(axis=0)
    margin = 0.08
    extent = max(float((hi - lo).max()), 1e-9)
    scale = min(
        w * (1 - 2 * margin) / max(hi[0] - lo[0], extent * 1e-3),
        h * (1 - 2 * margin) / max(hi[1] - lo[1], extent * 1e-3),
    )
    center = (lo + hi) / 2

    def to_screen(world):
        out = np.empty(world.shape[:-1] + (3,))
        out[..., 0] = (world @ right - center[0]) * scale + w / 2
        out[..., 1] = h / 2 - (world @ up - center[1]) * scale
        out[..., 2] = world @ forward
        return out

    faces, colours, segments = [], [], []
    for part, triangles, part_edges, capped in batches:
        normals = np.cross(
            triangles[:, 1] - triangles[:, 0], triangles[:, 2] - triangles[:, 0]
        )
        lengths = np.linalg.norm(normals, axis=1)
        valid = lengths > 0
        normals[valid] /= lengths[valid, None]
        # Meshes from booleans occasionally carry flipped facets; shade both sides.
        shade = np.abs(normals @ light)
        base = np.array(part["color"][:3])
        rgb = np.clip(
            base[None, :] * (0.28 + 0.72 * shade[:, None]) + 0.08 * shade[:, None] ** 8,
            0,
            1,
        )
        if capped:
            # Cut faces lie on the clip plane: draw them lighter so sections read.
            on_plane = np.abs(triangles.mean(axis=1) @ clip[0] - clip[1]) < 1e-5
            rgb[on_plane] = np.clip(base * 0.55 + 0.35, 0, 1)
        faces.append(to_screen(triangles[valid]))
        colours.append((rgb[valid] * 255).astype(np.uint8))
        if edges:
            segments.append(part_edges)
    pixels, depth = rasterize(
        np.concatenate(faces), np.concatenate(colours), w, h, BACKGROUND
    )
    image = Image.fromarray(pixels)
    if segments:
        draw_edges(
            ImageDraw.Draw(image),
            depth,
            to_screen(np.concatenate(segments)),
            extent * 2e-3,
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


def render(
    view, output: Path, views=("sheet",), size=(960, 720), title=None, clip=None
):
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
                render_solid(view, VIEWS[name], tile, label, clip=clip),
                ((index % 2) * tile[0], (index // 2) * tile[1]),
            )
        draw = ImageDraw.Draw(image)
        draw.line([(tile[0], 0), (tile[0], image.height)], fill=MUTED)
        draw.line([(0, tile[1]), (image.width, tile[1])], fill=MUTED)
    else:
        tiles = [
            render_solid(
                view, direction_of(v), size, f"{title} - {v}" if title else v, clip=clip
            )
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
    view_path: Path,
    output: Path,
    views=("sheet",),
    size=(960, 720),
    title=None,
    clip=None,
):
    return render(json.loads(view_path.read_text()), output, views, size, title, clip)
