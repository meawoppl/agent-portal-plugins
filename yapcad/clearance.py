"""Clearance between part groups of a run (view.json).

Groups select parts by case-insensitive substring of the part name or
material, comma-separated. Distances are exact mesh-to-mesh minima: closed
parts use manifold3d's BVH ``min_gap``; open meshes fall back to a
subdivided KD-tree search measuring candidate triangle pairs exactly
(vertex-face, edge-edge, and edge-through-face crossing).
Interference: closed parts whose boxes overlap are intersected with
manifold3d and any shared volume is reported.
"""

from __future__ import annotations

import base64

import numpy as np


def _triangles(part):
    data = np.frombuffer(base64.b64decode(part["positions"]), dtype=np.float32)
    return data.reshape(-1, 3, 3).astype(np.float64)


def _matches(part, selectors):
    text = f"{part.get('name') or ''} {part.get('material') or ''}".lower()
    return any(s and s.lower() in text for s in selectors)


def _selectors(spec):
    return [s.strip() for s in (spec or "").split(",") if s.strip()]


def _subdivide(tris, max_edge):
    """Split triangles until no edge exceeds ``max_edge`` (midpoint split of
    the longest edge); the surface is unchanged."""
    out = []
    work = tris
    while len(work):
        e = np.stack(
            [
                np.linalg.norm(work[:, 1] - work[:, 0], axis=1),
                np.linalg.norm(work[:, 2] - work[:, 1], axis=1),
                np.linalg.norm(work[:, 0] - work[:, 2], axis=1),
            ],
            axis=1,
        )
        small = e.max(axis=1) <= max_edge
        out.append(work[small])
        big = work[~small]
        if not len(big):
            break
        k = e[~small].argmax(axis=1)
        # Rotate so the longest edge is v0-v1, then split it at its midpoint.
        r = big[np.arange(len(big))[:, None], (k[:, None] + np.arange(3)) % 3]
        m = (r[:, 0] + r[:, 1]) / 2
        work = np.concatenate(
            [np.stack([r[:, 0], m, r[:, 2]], 1), np.stack([m, r[:, 1], r[:, 2]], 1)]
        )
    return np.concatenate(out) if out else tris[:0]


def segment_segment(p1, q1, p2, q2):
    """Closest distance between segment pairs (Ericson 5.1.9), vectorized."""
    d1, d2, r = q1 - p1, q2 - p2, p1 - p2
    a, e, f = (d1 * d1).sum(1), (d2 * d2).sum(1), (d2 * r).sum(1)
    c, b = (d1 * r).sum(1), (d1 * d2).sum(1)
    denom = a * e - b * b
    with np.errstate(divide="ignore", invalid="ignore"):
        s = np.where(denom > 1e-18, np.clip((b * f - c * e) / denom, 0, 1), 0.0)
        t = np.where(e > 1e-18, (b * s + f) / e, 0.0)
        s = np.where(t < 0, np.where(a > 1e-18, np.clip(-c / a, 0, 1), 0.0), s)
        s = np.where(t > 1, np.where(a > 1e-18, np.clip((b - c) / a, 0, 1), 0.0), s)
    t = np.clip(t, 0, 1)
    c1, c2 = p1 + d1 * s[:, None], p2 + d2 * t[:, None]
    return np.linalg.norm(c1 - c2, axis=1), c1, c2


def segment_crosses(p, q, tris):
    """Whether each segment p-q passes through its triangle (Moller-Trumbore)."""
    a, b, c = tris[:, 0], tris[:, 1], tris[:, 2]
    d = q - p
    e1, e2 = b - a, c - a
    h = np.cross(d, e2)
    det = (e1 * h).sum(1)
    ok = np.abs(det) > 1e-14
    inv = np.where(ok, 1.0 / np.where(ok, det, 1.0), 0.0)
    s = p - a
    u = inv * (s * h).sum(1)
    qv = np.cross(s, e1)
    v = inv * (d * qv).sum(1)
    t = inv * (e2 * qv).sum(1)
    return ok & (u >= 0) & (v >= 0) & (u + v <= 1) & (t >= 0) & (t <= 1)


def point_triangle_distance(p, tris):
    """Exact distance from points ``p`` (one per triangle, or one shared) to
    triangles; returns (d, closest). Ericson, Real-Time Collision Detection
    5.1.5."""
    a, b, c = tris[:, 0], tris[:, 1], tris[:, 2]
    ab, ac, ap = b - a, c - a, p - a
    d1, d2 = (ab * ap).sum(1), (ac * ap).sum(1)
    bp = p - b
    d3, d4 = (ab * bp).sum(1), (ac * bp).sum(1)
    cp = p - c
    d5, d6 = (ab * cp).sum(1), (ac * cp).sum(1)
    va = d3 * d6 - d5 * d4
    vb = d5 * d2 - d1 * d6
    vc = d1 * d4 - d3 * d2
    denom = va + vb + vc
    with np.errstate(divide="ignore", invalid="ignore"):
        v = np.where(denom != 0, vb / denom, 0.0)
        w = np.where(denom != 0, vc / denom, 0.0)
        t_ab = np.where(d1 - d3 != 0, d1 / (d1 - d3), 0.0)
        t_ac = np.where(d2 - d6 != 0, d2 / (d2 - d6), 0.0)
        t_bc = np.where(
            (d4 - d3) + (d5 - d6) != 0, (d4 - d3) / ((d4 - d3) + (d5 - d6)), 0.0
        )
    closest = a + ab * v[:, None] + ac * w[:, None]
    edge_bc = (va <= 0) & ((d4 - d3) >= 0) & ((d5 - d6) >= 0)
    edge_ac = (vb <= 0) & (d2 >= 0) & (d6 <= 0)
    edge_ab = (vc <= 0) & (d1 >= 0) & (d3 <= 0)
    closest = np.where(edge_bc[:, None], b + (c - b) * t_bc[:, None], closest)
    closest = np.where(edge_ac[:, None], a + ac * t_ac[:, None], closest)
    closest = np.where(edge_ab[:, None], a + ab * t_ab[:, None], closest)
    closest = np.where(((d6 >= 0) & (d5 <= d6))[:, None], c, closest)
    closest = np.where(((d3 >= 0) & (d4 <= d3))[:, None], b, closest)
    closest = np.where(((d1 <= 0) & (d2 <= 0))[:, None], a, closest)
    return np.linalg.norm(closest - p, axis=1), closest


def triangle_distance(ta, tb):
    """Exact distances between paired triangles; returns (d, point_a, point_b)."""
    best = np.full(len(ta), np.inf)
    pa = ta[:, 0].copy()
    pb = tb[:, 0].copy()

    def keep(d, x, y):
        better = d < best
        best[better], pa[better], pb[better] = d[better], x[better], y[better]

    for i in range(3):
        d, near = point_triangle_distance(ta[:, i], tb)
        keep(d, ta[:, i], near)
        d, near = point_triangle_distance(tb[:, i], ta)
        keep(d, near, tb[:, i])
    for i in range(3):
        for j in range(3):
            d, x, y = segment_segment(
                ta[:, i], ta[:, (i + 1) % 3], tb[:, j], tb[:, (j + 1) % 3]
            )
            keep(d, x, y)
    for i in range(3):
        hit = segment_crosses(ta[:, i], ta[:, (i + 1) % 3], tb) | segment_crosses(
            tb[:, i], tb[:, (i + 1) % 3], ta
        )
        best[hit] = 0.0
    return best, pa, pb


def pair_distance(ta, tb, cell, cap=np.inf):
    """Exact minimum distance between two triangle soups.

    Large triangles are subdivided to ``cell`` so centres plus radii bound
    each triangle tightly. Each A triangle is measured exactly against its
    nearest B centres for an upper bound; only A triangles whose lower bound
    (centre gap minus both radii) can beat it are searched in a ball.
    ``cap``: stop early once the pair is known to be farther than this.
    """
    from scipy.spatial import cKDTree

    ta, tb = _subdivide(ta, cell), _subdivide(tb, cell)
    if len(tb) > len(ta):
        d, x, y = pair_distance(tb, ta, cell, cap)
        return d, y, x
    ca, cb = ta.mean(1), tb.mean(1)
    ra = np.linalg.norm(ta - ca[:, None], axis=2).max(1)
    rb = np.linalg.norm(tb - cb[:, None], axis=2).max(1)
    rb_max = float(rb.max())
    tree_b = cKDTree(cb)
    k = min(4, len(cb))
    gap, near = tree_b.query(ca, k=k)
    gap, near = gap.reshape(len(ca), k), near.reshape(len(ca), k)
    lower = gap[:, 0] - ra - rb_max
    if lower.min() > cap:
        return (
            float(lower.min()),
            ca[int(lower.argmin())],
            cb[int(near[lower.argmin(), 0])],
        )
    # Upper bound: exact distance for the most promising triangles' neighbours.
    seed = np.argsort(lower)[:2048]
    ia, ib = np.repeat(seed, k), near[seed].ravel()
    d, x, y = triangle_distance(ta[ia], tb[ib])
    i = int(np.argmin(d))
    best = (float(d[i]), x[i], y[i])
    survivors = np.flatnonzero(lower <= best[0] + 1e-9)
    for start in range(0, len(survivors), 4096):
        chunk = survivors[start : start + 4096]
        balls = tree_b.query_ball_point(ca[chunk], best[0] + ra[chunk] + rb_max)
        ia = np.repeat(chunk, [len(b) for b in balls])
        ib = np.fromiter((j for b in balls for j in b), dtype=np.int64, count=len(ia))
        if not len(ia):
            continue
        tight = (
            np.linalg.norm(ca[ia] - cb[ib], axis=1) - ra[ia] - rb[ib] <= best[0] + 1e-9
        )
        ia, ib = ia[tight], ib[tight]
        for lo in range(0, len(ia), 200_000):
            d, x, y = triangle_distance(
                ta[ia[lo : lo + 200_000]], tb[ib[lo : lo + 200_000]]
            )
            i = int(np.argmin(d))
            if d[i] < best[0]:
                best = (float(d[i]), x[i], y[i])
    return best


def _crop(tris, lo, hi, reach):
    """Triangles whose boxes come within ``reach`` of the box lo..hi."""
    tlo, thi = tris.min(1), tris.max(1)
    gap = np.maximum(0, np.maximum(lo - thi, tlo - hi))
    return tris[np.linalg.norm(gap, axis=1) <= reach]


def local_distance(ta, tb, cell=2.0, cap=np.inf, budget=20000):
    """``pair_distance`` restricted to the parts' facing regions, with the
    subdivision cell grown so each side stays within ``budget`` triangles."""
    from scipy.spatial import cKDTree

    va, vb = ta.reshape(-1, 3), tb.reshape(-1, 3)
    upper = float(cKDTree(vb).query(va)[0].min())  # vertex pairs bound it above
    reach = min(upper, cap) + 1e-6
    ta = _crop(ta, vb.min(0), vb.max(0), reach)
    tb = _crop(tb, va.min(0), va.max(0), reach)
    if not len(ta) or not len(tb):
        return np.inf, va[0], vb[0]
    sizes = []
    for t in (ta, tb):
        area = (
            0.5
            * np.linalg.norm(
                np.cross(t[:, 1] - t[:, 0], t[:, 2] - t[:, 0]), axis=1
            ).sum()
        )
        sizes.append(np.sqrt(2.0 * area / budget))
    # Candidates per triangle scale like (gap / cell)^2, so let the cell grow
    # with the upper bound; subdivision never changes the surface, so the
    # result stays exact.
    return pair_distance(ta, tb, max(cell, *sizes, 0.5 * min(upper, cap)), cap)


def _manifold(tris):
    from manifold3d import Manifold, Mesh

    unique, index = np.unique(
        np.round(tris.reshape(-1, 3), 6), axis=0, return_inverse=True
    )
    m = Manifold(
        Mesh(
            vert_properties=np.ascontiguousarray(unique, dtype=np.float32),
            tri_verts=np.ascontiguousarray(index.reshape(-1, 3), dtype=np.uint32),
        )
    )
    return m if m.status().name == "NoError" and not m.is_empty() else None


def locate(ta, tb, gap, seeds=6, rounds=6):
    """Closest points for a known gap without subdividing: seed from the
    nearest vertex/centroid pairs, then alternate exact projections onto
    each mesh (closest point on B, then back onto A). Converges onto the
    closest region; reported as an approximate location."""
    from scipy.spatial import cKDTree

    va, vb = ta.reshape(-1, 3), tb.reshape(-1, 3)
    reach = gap + 1e-6
    near_a = _crop(ta, vb.min(0), vb.max(0), reach)
    near_b = _crop(tb, va.min(0), va.max(0), reach)
    if not len(near_a) or not len(near_b):
        return va[0], vb[0]
    pa = np.concatenate([near_a.reshape(-1, 3), near_a.mean(1)])
    pb = np.concatenate([near_b.reshape(-1, 3), near_b.mean(1)])
    d, j = cKDTree(pb).query(pa)
    best = (np.inf, pa[0], pb[0])
    for i in np.argsort(d)[:seeds]:
        p = pa[i]
        for _ in range(rounds):
            dq, q = point_triangle_distance(p, near_b)
            q = q[int(np.argmin(dq))]
            dp, p = point_triangle_distance(q, near_a)
            p = p[int(np.argmin(dp))]
        dist = float(np.linalg.norm(p - q))
        if dist < best[0]:
            best = (dist, p, q)
    return best[1], best[2]


def clearance(view, a, b=None, exclude=None, cell=2.0, minimum=0.0, top=8):
    sel_a, sel_b, sel_x = _selectors(a), _selectors(b), _selectors(exclude)
    if not sel_a:
        raise ValueError("clearance needs --a: part name or material substrings")
    parts = [p for p in view.get("parts", []) if not _matches(p, sel_x)]
    group_a = [p for p in parts if _matches(p, sel_a)]
    group_b = [
        p for p in parts if p not in group_a and (not sel_b or _matches(p, sel_b))
    ]
    if not group_a or not group_b:
        raise ValueError(
            f"empty group: a matched {len(group_a)} parts, b matched {len(group_b)}; parts are "
            + ", ".join(
                f"{p['name']} ({p.get('material')})" for p in view.get("parts", [])
            )
        )
    tris = {id(p): _triangles(p) for p in group_a + group_b}
    solids = {k: _manifold(t) for k, t in tris.items()}
    boxes = {k: (t.min((0, 1)), t.max((0, 1))) for k, t in tris.items()}
    pairs = []
    for part_a in group_a:
        for part_b in group_b:
            (la, ha), (lb, hb) = boxes[id(part_a)], boxes[id(part_b)]
            gap = np.maximum(0, np.maximum(la - hb, lb - ha))
            pairs.append((float(np.linalg.norm(gap)), part_a, part_b))
    pairs.sort(key=lambda r: r[0])
    results, methods = [], set()
    for box_gap, part_a, part_b in pairs:
        worst = (
            sorted(r["distance_mm"] for r in results)[top - 1]
            if len(results) >= top
            else np.inf
        )
        # Box gaps bound the distance below: nothing later can make the list.
        if box_gap > worst:
            break
        ma, mb = solids[id(part_a)], solids[id(part_b)]
        ta, tb = tris[id(part_a)], tris[id(part_b)]
        if ma is not None and mb is not None:
            # manifold3d's BVH query is exact for closed solids.
            search = worst if np.isfinite(worst) else 1e9
            dist = float(ma.min_gap(mb, search + 1e-6))
            if dist > search:
                continue
            pa_pt = pb_pt = None  # located below, for reported pairs only
            method = "min_gap"
        else:
            dist, pa_pt, pb_pt = local_distance(ta, tb, cell, worst)
            if not np.isfinite(dist) or dist > worst:
                continue
            method = "triangles"
        methods.add(method)
        results.append(
            {
                "a": part_a["name"],
                "a_material": part_a.get("material"),
                "b": part_b["name"],
                "b_material": part_b.get("material"),
                "distance_mm": round(dist, 4),
                "near_a": pa_pt,
                "near_b": pb_pt,
                "method": method,
                "_tris": (tris[id(part_a)], tris[id(part_b)]),
            }
        )
    results.sort(key=lambda r: r["distance_mm"])
    results = results[:top]
    for r in results:
        ta, tb = r.pop("_tris")
        if r["near_a"] is None:
            r["near_a"], r["near_b"] = locate(ta, tb, r["distance_mm"])
        r["near_a"] = [round(float(v), 3) for v in r["near_a"]]
        r["near_b"] = [round(float(v), 3) for v in r["near_b"]]
    interference = []
    for part_a in group_a:
        for part_b in group_b:
            (la, ha), (lb, hb) = boxes[id(part_a)], boxes[id(part_b)]
            if (np.maximum(la, lb) > np.minimum(ha, hb)).any():
                continue
            ma, mb = solids[id(part_a)], solids[id(part_b)]
            if ma is None or mb is None:
                continue
            volume = (ma ^ mb).volume()
            if volume > 1e-6:
                interference.append(
                    {
                        "a": part_a["name"],
                        "b": part_b["name"],
                        "volume_mm3": round(volume, 4),
                    }
                )
    min_distance = results[0]["distance_mm"] if results else None
    if interference:
        status = "interference"
    elif min_distance is not None and min_distance <= 1e-6:
        status = "contact"
    elif min_distance is not None and min_distance < minimum:
        status = "too_close"
    else:
        status = "clear"
    open_parts = [p["name"] for p in group_a + group_b if solids[id(p)] is None]
    return {
        "status": status,
        "min_distance_mm": min_distance,
        "required_mm": minimum,
        "group_a": [p["name"] for p in group_a],
        "group_b": [p["name"] for p in group_b],
        "excluded": [p["name"] for p in view.get("parts", []) if _matches(p, sel_x)],
        "closest": results[:top],
        "interference": interference,
        "open_parts": open_parts,
        "note": "distances are exact; near_a/near_b locate the closest region (approximate)"
        + (
            "; open (non-manifold) parts use the slower triangle search and skip the interference check"
            if open_parts
            else ""
        ),
    }
