"""yapCAD worker. Runs inside the yapCAD interpreter (the plugin venv or
YAPCAD_PYTHON, e.g. a conda env with pythonocc-core) and only needs yapCAD and
numpy. The workbench server owns run directories, timeouts and rendering."""

from __future__ import annotations

import argparse
import base64
import json
import math
import re
import sys
import time
import traceback
import warnings
from pathlib import Path

warnings.filterwarnings("ignore", message=".*BREP support.*")
warnings.filterwarnings("ignore", category=DeprecationWarning)

import numpy as np  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parent))
from meshutil import feature_edges  # noqa: E402

PALETTE = [
    [0.48, 0.64, 0.97],
    [0.62, 0.81, 0.42],
    [0.88, 0.69, 0.41],
    [0.73, 0.60, 0.97],
    [0.49, 0.81, 1.00],
    [0.97, 0.46, 0.56],
]
MAX_TRIANGLES = 2_000_000


def emit(data, out: Path | None = None):
    text = json.dumps(data, indent=2, default=str)
    if out:
        temporary = out.with_suffix(".tmp")
        temporary.write_text(text)
        temporary.replace(out)
    else:
        print(text)


def capabilities():
    import yapcad

    info = {
        "yapcad": getattr(yapcad, "__version__", "unknown"),
        "python": sys.version.split()[0],
    }
    import yapcad_patches

    info["patches"] = yapcad_patches.apply()
    info["brep"] = bool(yapcad.has_brep())
    for module in ("manifold3d", "trimesh", "pymeshfix", "ezdxf"):
        try:
            __import__(module)
            info[module] = True
        except ImportError:
            info[module] = False
    return info


# ---------------------------------------------------------------- DSL source


def literal(expression, source: str):
    """Return a JSON value for a default expression, or its source text."""
    from yapcad.dsl import ast

    if expression is None:
        return None
    if isinstance(expression, ast.Literal):
        return expression.value
    if isinstance(expression, ast.ListLiteral):
        values = [literal(e, source) for e in expression.elements]
        if all(not isinstance(v, dict) for v in values):
            return values
    if isinstance(expression, ast.UnaryOp) and isinstance(
        expression.operand, ast.Literal
    ):
        value = expression.operand.value
        if isinstance(value, (int, float)) and "MINUS" in str(expression.operator):
            return -value
    span = getattr(expression, "span", None)
    if span is not None:
        return {"expression": source[span.start.offset : span.end.offset]}
    return {"expression": str(expression)}


def type_name(node) -> str:
    from yapcad.dsl.ast import GenericType, OptionalType, SimpleType

    if node is None:
        return "any"
    if isinstance(node, SimpleType):
        return node.name
    if isinstance(node, GenericType):
        return f"{node.name}<{', '.join(type_name(a) for a in node.type_args)}>"
    if isinstance(node, OptionalType):
        return f"{type_name(node.inner)}?"
    return str(node)


def location(error: Exception):
    match = re.match(r"\s*(\d+):(\d+):", str(error))
    return (int(match[1]), int(match[2])) if match else (None, None)


def parse_source(source: str):
    from yapcad.dsl import parse, tokenize

    return parse(tokenize(source), source=source)


def describe(path: Path):
    source = path.read_text()
    try:
        module = parse_source(source)
    except Exception as error:  # noqa: BLE001 - parser errors are user data
        line, column = location(error)
        return {
            "file": str(path),
            "ok": False,
            "error": str(error),
            "line": line,
            "column": column,
            "commands": [],
        }
    commands = []
    for command in module.commands:
        commands.append(
            {
                "name": command.name,
                "line": command.span.start.line if command.span else None,
                "return_type": type_name(command.return_type),
                "meta": command.meta_hint or {},
                "params": [
                    {
                        "name": p.name,
                        "type": type_name(p.type_annotation),
                        "default": literal(p.default_value, source),
                        "required": p.default_value is None,
                        "ui": p.ui_hint or {},
                    }
                    for p in command.parameters
                ],
            }
        )
    return {"file": str(path), "ok": True, "module": module.name, "commands": commands}


def check_file(path: Path):
    from yapcad.dsl import check

    source = path.read_text()
    diagnostics = []
    try:
        module = parse_source(source)
        result = check(module)
        for d in result.diagnostics:
            diagnostics.append(
                {
                    "severity": d.severity.value,
                    "code": d.code,
                    "message": d.message,
                    "line": d.span.start.line,
                    "column": d.span.start.column,
                    "hints": list(d.hints or []),
                }
            )
        commands = len(module.commands)
    except Exception as error:  # noqa: BLE001
        line, column = location(error)
        diagnostics.append(
            {
                "severity": "error",
                "code": "parse",
                "message": str(error),
                "line": line,
                "column": column,
                "hints": [],
            }
        )
        commands = 0
    return {
        "file": str(path),
        "ok": not any(d["severity"] == "error" for d in diagnostics),
        "commands": commands,
        "diagnostics": diagnostics,
    }


def coerce(params: dict, command: dict | None):
    """Coerce JSON values to declared parameter types (int -> float etc.)."""
    if not command:
        return params
    declared = {p["name"]: p["type"] for p in command["params"]}
    unknown = set(params) - set(declared)
    if unknown:
        raise ValueError(
            f"Unknown parameter(s) for {command['name']}: {', '.join(sorted(unknown))}"
        )
    result = {}
    for name, value in params.items():
        kind = declared[name].rstrip("?")
        if (
            kind == "float"
            and isinstance(value, (int, float))
            and not isinstance(value, bool)
        ):
            value = float(value)
        elif kind == "int" and isinstance(value, float) and value.is_integer():
            value = int(value)
        elif kind == "bool" and isinstance(value, str):
            value = value.lower() in ("1", "true", "yes", "on")
        elif kind == "string" and not isinstance(value, str):
            value = str(value)
        result[name] = value
    return result


# ---------------------------------------------------------- geometry to view


def solids_of(geometry):
    from yapcad.geom3d import issolid, issurface

    if issolid(geometry) or issurface(geometry):
        return [geometry]
    if (
        isinstance(geometry, list)
        and geometry
        and all(issolid(g) or issurface(g) for g in geometry)
    ):
        return list(geometry)
    return []


def pieces(solids, name):
    """Viewer parts: one per solid, or one per surface for compound solids
    whose surfaces carry their own entity ids (e.g. packaged assemblies)."""
    result = []
    for index, entity in enumerate(solids):
        meta = part_meta(entity)
        base = meta.get("name") or (name if len(solids) == 1 else f"{name}-{index + 1}")
        surfaces = entity[1] if entity[0] == "solid" else []
        if len(surfaces) > 1 and all(
            (part_meta(s).get("entityId") or part_meta(s).get("id")) for s in surfaces
        ):
            for number, surface in enumerate(surfaces):
                result.append(
                    (
                        surface,
                        {**meta, **part_meta(surface)},
                        part_meta(surface).get("name") or f"{base}.{number + 1}",
                    )
                )
        else:
            result.append((entity, meta, base))
    return result


def part_meta(entity):
    meta = {}
    if (
        entity
        and entity[0] == "solid"
        and len(entity) > 4
        and isinstance(entity[4], dict)
    ):
        meta = entity[4]
    elif (
        entity
        and entity[0] == "surface"
        and len(entity) > 6
        and isinstance(entity[6], dict)
    ):
        meta = entity[6]
    return meta


def color_of(meta, index):
    material = meta.get("material") if isinstance(meta, dict) else None
    if isinstance(material, dict):
        for candidate in (
            (material.get("visual", {}) or {}).get("color"),
            material.get("color"),
        ):
            if (
                isinstance(candidate, list)
                and len(candidate) >= 3
                and all(isinstance(c, (int, float)) for c in candidate[:3])
            ):
                return [float(c) for c in candidate[:3]]
    return PALETTE[index % len(PALETTE)]


def b64(array):
    return base64.b64encode(np.ascontiguousarray(array).tobytes()).decode()


def mesh_arrays(entity):
    from yapcad.mesh import mesh_view

    rows = []
    for _normal, v0, v1, v2 in mesh_view(entity):
        rows.append((*v0[:3], *v1[:3], *v2[:3]))
        if len(rows) > MAX_TRIANGLES:
            raise ValueError(
                f"Mesh exceeds {MAX_TRIANGLES} triangles; coarsen the model or export directly"
            )
    # Snap to a 1e-6 grid so coincident vertices are bit-identical in float32
    # (avoids hairline rasterization cracks between neighbouring facets).
    return np.round(np.asarray(rows, dtype=np.float64).reshape(-1, 3, 3), 6)


def mesh_stats(triangles):
    if not len(triangles):
        return {"triangles": 0}
    a, b, c = triangles[:, 0], triangles[:, 1], triangles[:, 2]
    cross = np.cross(b - a, c - a)
    area = float(np.linalg.norm(cross, axis=1).sum() / 2)
    signed = np.einsum("ij,ij->i", a, np.cross(b, c)) / 6.0
    volume = float(signed.sum())
    centroid = (
        ((a + b + c) / 4.0 * signed[:, None]).sum(axis=0) / volume
        if abs(volume) > 1e-12
        else (a + b + c).mean(axis=0) / 3
    )
    points = triangles.reshape(-1, 3)
    stats = {
        "triangles": int(len(triangles)),
        "area": area,
        "volume": abs(volume),
        "inverted": volume < 0,
        "centroid": [float(x) for x in centroid],
        "bbox": [points.min(axis=0).tolist(), points.max(axis=0).tolist()],
    }
    # Edge manifoldness without optional dependencies.
    keys = np.round(points / 1e-6).astype(np.int64)
    _, vertex_ids = np.unique(keys, axis=0, return_inverse=True)
    vertex_ids = vertex_ids.reshape(-1, 3)
    edges = np.sort(
        np.concatenate(
            [vertex_ids[:, [0, 1]], vertex_ids[:, [1, 2]], vertex_ids[:, [2, 0]]]
        ),
        axis=1,
    )
    _, counts = np.unique(edges, axis=0, return_counts=True)
    stats["boundary_edges"] = int((counts == 1).sum())
    stats["nonmanifold_edges"] = int((counts > 2).sum())
    stats["watertight"] = (
        stats["boundary_edges"] == 0 and stats["nonmanifold_edges"] == 0
    )
    try:
        import trimesh

        mesh = trimesh.Trimesh(
            vertices=points, faces=np.arange(len(points)).reshape(-1, 3), process=True
        )
        stats["bodies"] = int(mesh.body_count)
        stats["euler_number"] = int(mesh.euler_number)
        stats["is_volume"] = bool(mesh.is_volume)
    except Exception:  # noqa: BLE001 - optional diagnostics
        pass
    return stats


def sample_2d(geometry):
    """Flatten 2D yapCAD geometry into polylines [[x, y], ...]."""
    from yapcad import geom

    paths = []

    def add(points, closed=False, kind="path"):
        pts = [[float(p[0]), float(p[1])] for p in points]
        if len(pts) >= 2:
            if not closed and math.dist(pts[0], pts[-1]) < 1e-9:
                closed = True
            paths.append({"points": pts, "closed": closed, "kind": kind})

    def visit(item, depth=0):
        if depth > 64:
            return
        if geom.ispoint(item):
            paths.append(
                {
                    "points": [[float(item[0]), float(item[1])]],
                    "closed": False,
                    "kind": "point",
                }
            )
        elif geom.isline(item):
            add(item[:2], kind="line")
        elif geom.iscircle(item):
            add([geom.sample(item, i / 96) for i in range(97)], True, "circle")
        elif geom.isarc(item):
            add([geom.sample(item, i / 64) for i in range(65)], kind="arc")
        elif any(f(item) for f in (geom.isellipse, geom.iscatmullrom, geom.isnurbs)):
            add([geom.sample(item, i / 128) for i in range(129)], kind=item[0])
        elif geom.ispoly(item):
            add(item, kind="polygon")
        elif isinstance(item, list):
            for child in item:
                visit(child, depth + 1)

    visit(geometry)
    return paths


def is_2d(geometry):
    try:
        from yapcad.ezdxf_exporter import is_2d_geometry

        return bool(is_2d_geometry(geometry))
    except Exception:  # noqa: BLE001
        return False


def assembly_pieces(assembly):
    """Positioned per-instance solids and world datums of a retained assembly."""
    result, datums = [], []
    for part_name, solid in assembly.positioned_parts().items():
        result.append((solid, part_meta(solid), part_name))
        for datum_name in getattr(assembly.parts[part_name], "datums", {}) or {}:
            try:
                datum = assembly.get_transformed_datum(part_name, datum_name)
            except Exception:  # noqa: BLE001 - datums are decoration
                continue
            direction = getattr(datum, "direction", None) or getattr(
                datum, "normal", None
            )
            datums.append(
                {
                    "part": part_name,
                    "id": datum_name,
                    "kind": getattr(getattr(datum, "datum_type", None), "value", None),
                    "origin": [float(v) for v in datum.origin[:3]],
                    "direction": [float(v) for v in direction[:3]]
                    if direction is not None
                    else None,
                }
            )
    return result, datums


def retained_assembly(execution):
    value = getattr(getattr(execution, "emit_result", None), "value", None)
    annotations = getattr(value, "annotations", None)
    return annotations.get("assembly") if isinstance(annotations, dict) else None


def build_view(geometry, name="model", assembly=None):
    """Serialize geometry for the browser viewer and the PNG renderer."""
    solids = solids_of(geometry)
    view = {"kind": "empty", "parts": [], "paths": [], "datums": []}
    assembly_datums = None
    if solids and assembly is not None:
        try:
            parts_list, assembly_datums = assembly_pieces(assembly)
        except Exception:  # noqa: BLE001 - fall back to the compound
            parts_list = None
        if parts_list:
            view["assembly"] = {
                "name": getattr(assembly, "name", None),
                "parts": [p[2] for p in parts_list],
            }
    else:
        parts_list = None
    if solids:
        view["kind"] = "solid"
        all_triangles = []
        for index, (entity, meta, part_name) in enumerate(
            parts_list or pieces(solids, name)
        ):
            triangles = mesh_arrays(entity)
            part = {
                "name": str(part_name),
                "color": color_of(meta, index),
                "positions": b64(triangles.astype(np.float32).reshape(-1)),
                "edges": b64(feature_edges(triangles).astype(np.float32).reshape(-1)),
                "stats": mesh_stats(triangles),
                "material": (meta.get("material") or {}).get("name")
                if isinstance(meta.get("material"), dict)
                else meta.get("material")
                if isinstance(meta.get("material"), str)
                else None,
                "tags": meta.get("tags") or [],
            }
            view["parts"].append(part)
            for datum in (meta.get("assembly") or {}).get("datums") or []:
                origin = datum.get("origin_mm")
                if origin is None and "R_mm" in datum:
                    origin = [datum["R_mm"], 0.0, datum.get("z_mm", 0.0)]
                if origin is not None:
                    view["datums"].append(
                        {
                            "part": part["name"],
                            "id": datum.get("id"),
                            "kind": datum.get("kind"),
                            "origin": origin,
                            "direction": datum.get("direction"),
                        }
                    )
            all_triangles.append(triangles)
        if assembly_datums is not None:
            view["datums"] = assembly_datums
        combined = (
            np.concatenate(all_triangles) if all_triangles else np.zeros((0, 3, 3))
        )
        view["stats"] = mesh_stats(combined)
        view["stats"]["parts"] = len(view["parts"])
        view["stats"]["volume"] = sum(
            p["stats"].get("volume", 0.0) for p in view["parts"]
        )
    elif is_2d(geometry) or isinstance(geometry, list):
        paths = sample_2d(geometry)
        if paths:
            view["kind"] = "2d"
            view["paths"] = paths
            points = np.array([p for path in paths for p in path["points"]])
            view["stats"] = {
                "paths": len(paths),
                "bbox": [points.min(axis=0).tolist(), points.max(axis=0).tolist()],
            }
    if view["kind"] == "empty" and geometry is not None:
        view["kind"] = "value"
        view["value"] = (
            geometry
            if isinstance(geometry, (int, float, str, bool))
            else repr(geometry)[:20000]
        )
    return view


def write_svg(paths, out: Path):
    points = np.array([p for path in paths for p in path["points"]])
    lo, hi = points.min(axis=0), points.max(axis=0)
    span = max(float((hi - lo).max()), 1e-9)
    pad = span * 0.05
    width, height = float(hi[0] - lo[0]) + 2 * pad, float(hi[1] - lo[1]) + 2 * pad
    stroke = span / 400
    body = []
    for path in paths:
        coords = " ".join(f"{x:.6g},{-y:.6g}" for x, y in path["points"])
        if len(path["points"]) == 1:
            x, y = path["points"][0]
            body.append(
                f'<circle cx="{x:.6g}" cy="{-y:.6g}" r="{stroke * 2:.6g}" fill="#e0af68"/>'
            )
        elif path["closed"]:
            body.append(f'<polygon points="{coords}"/>')
        else:
            body.append(f'<polyline points="{coords}"/>')
    out.write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{lo[0] - pad:.6g} {-hi[1] - pad:.6g} {width:.6g} {height:.6g}" '
        f'width="{width:.6g}mm" height="{height:.6g}mm">\n'
        f'<rect x="{lo[0] - pad:.6g}" y="{-hi[1] - pad:.6g}" width="{width:.6g}" height="{height:.6g}" fill="#1a1b26"/>\n'
        f'<g fill="none" stroke="#7aa2f7" stroke-width="{stroke:.6g}" stroke-linejoin="round">\n'
        + "\n".join(body)
        + "\n</g>\n</svg>\n"
    )


def write_stl_triangles(triangles, out: Path, name="yapCAD"):
    data = np.zeros(
        len(triangles),
        dtype=[("normal", "<f4", 3), ("vertices", "<f4", (3, 3)), ("attr", "<u2")],
    )
    if len(triangles):
        normals = np.cross(
            triangles[:, 1] - triangles[:, 0], triangles[:, 2] - triangles[:, 0]
        )
        lengths = np.linalg.norm(normals, axis=1, keepdims=True)
        data["normal"] = np.divide(
            normals, lengths, out=np.zeros_like(normals), where=lengths > 0
        )
        data["vertices"] = triangles
    with out.open("wb") as stream:
        stream.write(name.encode()[:80].ljust(80, b"\0"))
        stream.write(np.uint32(len(triangles)).tobytes())
        stream.write(data.tobytes())


def export(geometry, view, formats, out: Path, spec, notes):
    """Write requested interchange files; returns {format: filename}."""
    files = {}
    solids = solids_of(geometry)
    for fmt in formats:
        try:
            if fmt == "stl" and solids:
                if len(solids) == 1:
                    from yapcad.io import write_stl_brep

                    used = write_stl_brep(
                        solids[0],
                        str(out / "model.stl"),
                        fallback_to_mesh=not spec.get("strict_stl"),
                        validate_watertight=bool(spec.get("strict_stl")),
                    )
                    notes.append(
                        "STL: " + ("BREP tessellation" if used else "display mesh")
                    )
                else:
                    write_stl_triangles(
                        np.concatenate([mesh_arrays(s) for s in solids]),
                        out / "model.stl",
                    )
                    notes.append(f"STL: {len(solids)} shells written without union")
                files["stl"] = "model.stl"
            elif fmt == "step" and solids:
                target = solids[0]
                if len(solids) > 1:
                    from yapcad.geom3d import solid_boolean

                    for other in solids[1:]:
                        target = solid_boolean(target, other, "union")
                    notes.append(f"STEP: {len(solids)} solids unioned")
                strict = bool(spec.get("strict_step"))
                if strict or spec.get("step_format", "faceted") == "analytic":
                    from yapcad.io.step import write_step_analytic

                    analytic = write_step_analytic(
                        target, str(out / "model.step"), fallback_to_faceted=not strict
                    )
                    if not analytic and strict:
                        raise ValueError(
                            "analytic STEP requires a BREP solid and pythonocc-core"
                        )
                    notes.append(
                        "STEP: " + ("analytic BREP" if analytic else "faceted fallback")
                    )
                else:
                    from yapcad.io import write_step

                    write_step(target, str(out / "model.step"))
                    notes.append("STEP: faceted")
                files["step"] = "model.step"
            elif fmt == "dxf" and view["kind"] == "2d":
                from yapcad.ezdxf_exporter import write_dxf

                if not write_dxf(geometry, str(out / "model.dxf")):
                    raise ValueError("DXF exporter returned failure")
                files["dxf"] = "model.dxf"
            elif fmt == "svg" and view["kind"] == "2d":
                write_svg(view["paths"], out / "model.svg")
                files["svg"] = "model.svg"
            elif fmt not in ("stl", "step", "dxf", "svg"):
                notes.append(f"{fmt}: unknown export format")
            else:
                notes.append(f"{fmt.upper()}: not applicable to {view['kind']} results")
        except Exception as error:  # noqa: BLE001 - one export must not lose the build
            notes.append(f"{fmt.upper()} export failed: {error}")
    return files


# ------------------------------------------------------------------- actions


def trace_requires(source: str):
    """Record the failing require's line and text (yapCAD keeps only a message)."""
    from yapcad.dsl.runtime.interpreter import Interpreter

    original = Interpreter._execute_require
    if getattr(original, "traced", False):
        return

    def traced(self, stmt, ctx):
        before = len(ctx.require_failures)
        original(self, stmt, ctx)
        span = getattr(stmt, "span", None)
        for failure in ctx.require_failures[before:]:
            if span is not None and not failure.expression_text:
                line = (
                    source.splitlines()[span.start.line - 1].strip()
                    if span.start.line
                    else ""
                )
                failure.expression_text = f"line {span.start.line}: {line}"

    traced.traced = True
    Interpreter._execute_require = traced


def run_build(spec: dict, out: Path):
    from yapcad.dsl import compile_and_run

    started = time.perf_counter()
    path = Path(spec["file"])
    source = path.read_text()
    described = describe(path)
    command = next(
        (c for c in described["commands"] if c["name"] == spec["command"]), None
    )
    if described["ok"] and command is None:
        raise ValueError(f"Command {spec['command']} not found in {path.name}")
    params = coerce(spec.get("params") or {}, command)
    trace_requires(source)
    print(f"[yapcad] {path.name} {spec['command']} {json.dumps(params)}", flush=True)
    result = {"params": params, "notes": []}
    simplify = spec.get("simplify")
    if spec.get("kind") == "package":
        from yapcad.dsl.packaging import package_from_dsl
        from yapcad.package import PackageManifest, load_geometry
        from yapcad.package.validator import validate_package

        options = spec.get("package") or {}
        packaged = package_from_dsl(
            source,
            spec["command"],
            params,
            out / "package.ycpkg",
            name=options.get("name") or spec["command"].lower(),
            version=options.get("version") or "1.0.0",
            description=options.get("description"),
            component_exports=options.get("component_exports") or None,
            strict_component_stl=bool(spec.get("strict_stl")),
            overwrite=True,
            representation=spec.get("representation"),
            sdf_cell_mm=spec.get("sdf_cell"),
            sdf_simplify=simplify,
        )
        execution = packaged.execution_result
        if not packaged.success:
            result.update(success=False, error=packaged.error_message)
        else:
            manifest = PackageManifest.load(out / "package.ycpkg")
            geometry = load_geometry(manifest)
            ok, messages = validate_package(out / "package.ycpkg")
            result["package"] = {
                "path": "package.ycpkg",
                "valid": ok,
                "messages": messages,
                "manifest": manifest.data,
            }
    else:
        execution = compile_and_run(
            source,
            spec["command"],
            params,
            recursion_limit=spec.get("recursion_limit"),
            representation=spec.get("representation"),
            sdf_cell_mm=spec.get("sdf_cell"),
            sdf_simplify=simplify,
        )
        geometry = execution.geometry
    if execution is not None:
        result["require_failures"] = [
            {"message": f.message, "expression": f.expression_text}
            for f in (execution.require_failures or [])
        ]
        result["metadata"] = execution.metadata or {}
        result["provenance"] = (
            execution.provenance.to_dict() if execution.provenance else None
        )
        if not execution.success:
            result.update(success=False, error=execution.error_message)
    result["build_seconds"] = time.perf_counter() - started
    if result.get("success") is False:
        return result
    if isinstance(geometry, str) and geometry.lstrip().startswith("{"):
        # emit_assembly() returns a semantic assembly graph as JSON text.
        (out / "assembly.json").write_text(geometry)
        result["assembly"] = "assembly.json"
    view = build_view(
        geometry,
        spec["command"].lower(),
        retained_assembly(execution) if spec.get("kind") != "package" else None,
    )
    emit(view, out / "view.json")
    result["kind"] = view["kind"]
    if view.get("assembly"):
        result["assembly_parts"] = view["assembly"]["parts"]
    result["stats"] = view.get("stats", {})
    if view["kind"] == "value":
        result["value"] = view["value"]
    result["exports"] = export(
        geometry, view, spec.get("exports") or [], out, spec, result["notes"]
    )
    result["success"] = True
    result["total_seconds"] = time.perf_counter() - started
    return result


def run_import(spec: dict, out: Path):
    path = Path(spec["file"])
    suffix = path.suffix.lower()
    result = {"notes": []}
    if suffix == ".stl":
        from yapcad.io.stl import import_stl

        geometry = import_stl(str(path))
    elif suffix in (".step", ".stp"):
        from yapcad.io.step_importer import import_step

        geometry = import_step(str(path))
    elif suffix == ".ycpkg" or (path / "manifest.yaml").is_file():
        from yapcad.package import PackageManifest, load_geometry
        from yapcad.package.validator import validate_package

        manifest = PackageManifest.load(path)
        geometry = load_geometry(manifest)
        ok, messages = validate_package(path, strict=bool(spec.get("strict")))
        result["package"] = {
            "path": str(path),
            "valid": ok,
            "messages": messages,
            "manifest": manifest.data,
        }
    elif suffix == ".dxf":
        import ezdxf
        from ezdxf import path as dxfpath

        document = ezdxf.readfile(str(path))
        paths = []
        for entity in document.modelspace():
            try:
                curve = dxfpath.make_path(entity)
            except TypeError:
                continue
            points = [[float(v.x), float(v.y)] for v in curve.flattening(0.01)]
            if len(points) >= 2:
                paths.append(
                    {
                        "points": points,
                        "closed": bool(curve.is_closed),
                        "kind": entity.dxftype().lower(),
                        "layer": entity.dxf.layer,
                    }
                )
        view = {"kind": "2d", "paths": paths, "parts": [], "datums": []}
        pts = (
            np.array([p for item in paths for p in item["points"]])
            if paths
            else np.zeros((1, 2))
        )
        view["stats"] = {
            "paths": len(paths),
            "bbox": [pts.min(axis=0).tolist(), pts.max(axis=0).tolist()],
        }
        emit(view, out / "view.json")
        result.update(success=True, kind="2d", stats=view["stats"], exports={})
        return result
    else:
        raise ValueError(f"Unsupported file type: {path.name}")
    view = build_view(geometry, path.stem)
    emit(view, out / "view.json")
    result.update(success=True, kind=view["kind"], stats=view.get("stats", {}))
    result["exports"] = export(
        geometry, view, spec.get("exports") or [], out, spec, result["notes"]
    )
    return result


def api_reference(name: str | None):
    from yapcad.dsl import introspection as api

    if name:
        info = api.get_function_info(name) or api.get_type_info(name)
        if info is None:
            pattern = api.get_common_pattern(name)
            if pattern is None:
                raise ValueError(f"No DSL function, type or pattern named {name}")
            return {"pattern": name, "source": pattern}
        if api.get_type_info(name):
            info = {**info, "methods": api.get_methods_for_type(name)}
        return info
    reference = api.get_api_reference()
    reference["patterns"] = api.list_common_patterns()
    return reference


def main():
    parser = argparse.ArgumentParser(prog="yapcad-worker")
    parser.add_argument(
        "action",
        choices=("capabilities", "list", "check", "build", "import", "api", "validate"),
    )
    parser.add_argument("targets", nargs="*")
    parser.add_argument("--spec", type=Path, help="JSON build specification")
    parser.add_argument("--out", type=Path, help="Run directory")
    parser.add_argument("--strict", action="store_true")
    args = parser.parse_args()
    import yapcad_patches

    yapcad_patches.apply()
    if args.action == "capabilities":
        emit(capabilities())
    elif args.action == "list":
        emit([describe(Path(t)) for t in args.targets])
    elif args.action == "check":
        emit([check_file(Path(t)) for t in args.targets])
    elif args.action == "api":
        emit(api_reference(args.targets[0] if args.targets else None))
    elif args.action == "validate":
        from yapcad.package.validator import validate_package

        reports = []
        for target in args.targets:
            ok, messages = validate_package(Path(target), strict=args.strict)
            reports.append({"package": target, "valid": ok, "messages": messages})
        emit(reports)
        return 0 if all(r["valid"] for r in reports) else 1
    else:
        spec = json.loads(args.spec.read_text())
        try:
            result = (run_import if args.action == "import" else run_build)(
                spec, args.out
            )
        except Exception as error:  # noqa: BLE001 - reported to run.json
            traceback.print_exc()
            result = {"success": False, "error": f"{type(error).__name__}: {error}"}
        emit(result, args.out / "result.json")
        return 0 if result.get("success") else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
