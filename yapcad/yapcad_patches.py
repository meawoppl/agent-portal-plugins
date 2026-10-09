"""Fixes applied to the pinned yapCAD revision at worker start-up.

Each patch is small, idempotent and names the upstream behaviour it changes,
so it can be offered upstream and dropped when the pin moves past it.

- ``loop_reassignment``: inside a block (``for``/``if``), ``x = expr`` parses
  as an untyped declaration and shadowed the outer ``x`` until the block
  ended, so accumulation silently did nothing. An untyped declaration of a
  name already bound in an enclosing scope now updates that binding (typed
  ``x: float = ...`` still declares a new local).
- ``boolean_metadata``: ``union``/``difference``/``intersection`` (and the
  ``*_all`` forms, ``fillet``, ``chamfer``) returned solids without the first
  operand's metadata, dropping ``@meta(material=...)`` colours, tags and
  assembly datums. Results now inherit the first operand's metadata wherever
  they lack it.
- ``mesh_extrude``: ``extrude_region2d`` and ``helical_extrude`` raised
  "requires pythonocc-core" on mesh-only installs. Without OCC they now build
  watertight meshes with manifold3d (regions with holes, arcs sampled at
  ``ARC_STEP_DEG``, optional oblique direction, helical twist positive
  counter-clockwise viewed from +Z, matching the OCC path).
"""

from __future__ import annotations

import copy
import math

APPLIED: list[str] = []

# Metadata keys that identify a particular solid; never inherited.
IDENTITY_KEYS = {"id", "entityId", "brep"}
INHERITING = (
    "union",
    "difference",
    "intersection",
    "union_all",
    "difference_all",
    "intersection_all",
    "fillet",
    "chamfer",
)


def _loop_reassignment():
    from yapcad.dsl.runtime.interpreter import Interpreter

    original = Interpreter._execute_let
    if getattr(original, "patched", False):
        return

    def execute_let(self, stmt, ctx):
        if stmt.type_annotation is None and stmt.initializer is not None:
            scope = ctx.current_scope
            if (
                stmt.name not in scope.variables
                and scope.parent is not None
                and scope.parent.get(stmt.name) is not None
            ):
                value = self._evaluate(stmt.initializer, ctx)
                scope.parent.update(stmt.name, value)
                return
        original(self, stmt, ctx)

    execute_let.patched = True
    Interpreter._execute_let = execute_let
    APPLIED.append("loop_reassignment")


def _metadata(entity):
    if (
        isinstance(entity, list)
        and entity
        and entity[0] == "solid"
        and len(entity) > 4
        and isinstance(entity[4], dict)
    ):
        return entity[4]
    return None


def inherit_metadata(source, result):
    """Copy ``source`` solid metadata into ``result`` where it is missing."""
    src = _metadata(source)
    if not src or not (isinstance(result, list) and result and result[0] == "solid"):
        return result
    while len(result) < 5:
        result.append([] if len(result) < 4 else {})
    if not isinstance(result[4], dict):
        result[4] = {}
    dst = result[4]
    for key, value in src.items():
        if key in IDENTITY_KEYS:
            continue
        if (
            key not in dst
            or dst[key] in (None, "", [], {})
            or (key == "layer" and dst[key] == "default")
        ):
            dst[key] = copy.deepcopy(value)
    return result


def _boolean_metadata():
    from yapcad.dsl.runtime.builtins import get_builtin_registry

    registry = get_builtin_registry()
    for name in INHERITING:
        fn = registry.get_function(name)
        if fn is None or getattr(fn.implementation, "patched", False):
            continue
        original = fn.implementation

        def wrapped(*args, _original=original):
            out = _original(*args)
            first = args[0].data if args else None
            if (
                isinstance(first, list)
                and first
                and first[0] != "solid"
                and first
                and isinstance(first[0], list)
            ):
                first = first[0]  # list<solid> operand: inherit from its first solid
            inherit_metadata(first, out.data)
            return out

        wrapped.patched = True
        fn.implementation = wrapped
    APPLIED.append("boolean_metadata")


# Arc sampling for mesh extrusion (degrees per segment).
ARC_STEP_DEG = 3.0


def region_contours(profile):
    """Closed XY contours (outer boundaries and holes) of a region2d."""
    import numpy as np
    from yapcad.geom_util import geomlist2poly_components

    components = geomlist2poly_components(profile, minang=ARC_STEP_DEG, minlen=0.01)
    contours = []
    for outer, holes in components:
        for loop in [outer, *holes]:
            pts = np.array([[float(p[0]), float(p[1])] for p in loop], dtype=np.float64)
            if len(pts) > 1 and np.allclose(pts[0], pts[-1]):
                pts = pts[:-1]
            if len(pts) >= 3:
                contours.append(pts)
    if not contours:
        raise ValueError("extrude: the region has no closed contour")
    return contours


def mesh_extrude(
    profile, height, *, direction=None, twist_deg=0.0, segments=64, metadata=None
):
    """Watertight manifold3d extrusion of a region2d from z=0 to ``height``
    (or along ``direction``), optionally twisted about +Z."""
    from manifold3d import CrossSection, FillRule, Manifold
    from yapcad.boolean.manifold_engine import _from_manifold

    height = float(height)
    if height == 0:
        raise ValueError("extrude: height must be non-zero")
    section = CrossSection(region_contours(profile), FillRule.EvenOdd)
    if section.is_empty():
        raise ValueError("extrude: the region encloses no area")
    # Twisted sides are ruled between slices; at most 1 degree per slice keeps
    # the volume within about 1% of the exact helicoid.
    divisions = max(int(segments) - 1, math.ceil(abs(twist_deg))) if twist_deg else 0
    body = Manifold.extrude(section, 1.0, divisions, float(twist_deg))
    dx, dy, dz = [float(v) for v in (direction or (0.0, 0.0, 1.0))[:3]]
    if abs(dz) < 1e-12:
        raise ValueError("extrude: direction must have a z component")
    norm = (dx * dx + dy * dy + dz * dz) ** 0.5
    sx, sy, sz = dx / norm * height, dy / norm * height, dz / norm * height
    # Map (x, y, t) -> profile point + t * extrusion vector (a shear when oblique).
    body = body.transform(
        [[1.0, 0.0, sx, 0.0], [0.0, 1.0, sy, 0.0], [0.0, 0.0, sz, 0.0]]
    )
    result = _from_manifold(body, "extrude")
    result[3] = [
        "procedure",
        "extrude:manifold" if not twist_deg else "helical_extrude:manifold",
    ]
    if metadata:
        while len(result) < 5:
            result.append({})
        result[4] = dict(metadata)
    return result


def _mesh_extrude():
    import yapcad.geom3d_util as util

    occ = getattr(util, "occ_available", None)
    original_extrude = util.extrude_region2d
    original_helical = util.helical_extrude
    if getattr(original_extrude, "patched", False):
        return

    def extrude_region2d(profile, height, *, direction=None, metadata=None):
        if occ is not None and occ():
            return original_extrude(
                profile, height, direction=direction, metadata=metadata
            )
        return mesh_extrude(profile, height, direction=direction, metadata=metadata)

    def helical_extrude(
        profile,
        height,
        twist_angle_deg,
        *,
        auxiliary_radius=10.0,
        segments=64,
        metadata=None,
    ):
        if occ is not None and occ():
            return original_helical(
                profile,
                height,
                twist_angle_deg,
                auxiliary_radius=auxiliary_radius,
                segments=segments,
                metadata=metadata,
            )
        return mesh_extrude(
            profile,
            height,
            twist_deg=float(twist_angle_deg),
            segments=segments,
            metadata=metadata,
        )

    extrude_region2d.patched = helical_extrude.patched = True
    util.extrude_region2d = extrude_region2d
    util.helical_extrude = helical_extrude
    APPLIED.append("mesh_extrude")


def apply():
    """Apply every patch once; returns the names applied."""
    if not APPLIED:
        _loop_reassignment()
        _boolean_metadata()
        _mesh_extrude()
    return list(APPLIED)
