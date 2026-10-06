"""Mesh helpers shared by the worker (yapCAD env) and renderer (plugin env).
Only numpy is required."""

from __future__ import annotations

import math

import numpy as np


def feature_edges(triangles, angle=30.0):
    """Boundary, non-manifold and sharp (dihedral > angle) edges of a
    triangle soup (n, 3, 3), returned as segments (m, 2, 3)."""
    if not len(triangles):
        return np.zeros((0, 2, 3))
    normals = np.cross(
        triangles[:, 1] - triangles[:, 0], triangles[:, 2] - triangles[:, 0]
    )
    lengths = np.linalg.norm(normals, axis=1)
    normals = np.divide(
        normals,
        lengths[:, None],
        out=np.zeros_like(normals),
        where=lengths[:, None] > 0,
    )
    points = triangles.reshape(-1, 3)
    _, ids = np.unique(
        np.round(points / 1e-6).astype(np.int64), axis=0, return_inverse=True
    )
    ids = ids.reshape(-1, 3)
    corners = np.array([[0, 1], [1, 2], [2, 0]])
    edges = np.sort(ids[:, corners].reshape(-1, 2), axis=1)
    faces = np.repeat(np.arange(len(triangles)), 3)
    ends = triangles[:, corners].reshape(-1, 2, 3)
    _, group, counts = np.unique(edges, axis=0, return_inverse=True, return_counts=True)
    order = np.argsort(group.reshape(-1), kind="stable")
    starts = np.concatenate([[0], np.cumsum(counts)[:-1]])
    keep = counts != 2
    pairs = np.flatnonzero(counts == 2)
    a = faces[order[starts[pairs]]]
    b = faces[order[starts[pairs] + 1]]
    # Degenerate facets have no reliable normal; never call their edges sharp.
    sound = (lengths[a] > 0) & (lengths[b] > 0)
    sharp = sound & (
        np.einsum("ij,ij->i", normals[a], normals[b]) < math.cos(math.radians(angle))
    )
    keep[pairs[sharp]] = True
    return ends[order[starts[keep]]]
