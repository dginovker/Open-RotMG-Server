import math

import numpy as np
from scipy.ndimage import binary_dilation, distance_transform_edt, gaussian_filter1d, label
from skimage.morphology import skeletonize


def round_roads(cells, dictionary, width, height):
    indices = np.frombuffer(cells, dtype="<u2").reshape(height, width).copy()
    grounds = np.array([entry[0] for entry in dictionary])[indices]
    road = grounds == 0xd0
    skeleton = skeletonize(road)
    ys, xs = np.where(skeleton)
    nodes = set(zip(xs.tolist(), ys.tolist()))
    neighbors = {
        (x, y): [(x + dx, y + dy) for dx, dy in
                 [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)]
                 if (x + dx, y + dy) in nodes
                 and not (dx and dy and ((x + dx, y) in nodes or (x, y + dy) in nodes))]
        for x, y in sorted(nodes)
    }
    visited, paths = set(), []
    for start in sorted(nodes, key=lambda p: (len(neighbors[p]) == 2, p)):
        for current in neighbors[start]:
            edge = frozenset((start, current))
            if edge in visited:
                continue
            path, previous = [start], start
            visited.add(edge)
            while current != start and len(neighbors[current]) == 2:
                path.append(current)
                following = next(p for p in neighbors[current] if p != previous)
                visited.add(frozenset((current, following)))
                previous, current = current, following
            paths.append(path + [current])

    moved = {point: point for point in nodes}
    changed = np.zeros_like(road)
    for path in paths:
        if len(path) < 40:
            continue
        closed = path[0] == path[-1] and len(neighbors[path[0]]) == 2
        if closed:
            path = path[:-1]
        points = np.array(path, dtype=float)
        incoming, outgoing = np.roll(points, 12, axis=0) - points, np.roll(points, -12, axis=0) - points
        cosine = (incoming * outgoing).sum(axis=1) / np.linalg.norm(incoming, axis=1) / np.linalg.norm(outgoing, axis=1)
        bends = (cosine > -0.5).astype(float)
        if not closed:
            bends[:18] = bends[-18:] = 0
        mode = "wrap" if closed else "nearest"
        weight = np.minimum(1, gaussian_filter1d(bends, 4, mode=mode) * 4)
        if not closed:
            weight[:12] = weight[-12:] = 0
        # The imported raster lost renderRoads' spline controls: https://github.com/amitp/mapgen2/blob/d0b09469777fcaa211c31edcefa59f13f416141f/mapgen2.as#L661-L729
        rounded = points + (gaussian_filter1d(points, 8, axis=0, mode=mode) - points) * weight[:, None]
        for original, point in zip(path, rounded):
            moved[original] = point
            if np.linalg.norm(point - original) > 0.2:
                changed[original[1], original[0]] = True

    radius = distance_transform_edt(road)
    painted = np.zeros_like(road)
    for x, y in sorted(nodes):
        point = moved[x, y]
        r = radius[y, x]
        for yy in range(max(0, math.floor(point[1] - r)), min(height, math.ceil(point[1] + r) + 1)):
            for xx in range(max(0, math.floor(point[0] - r)), min(width, math.ceil(point[0] + r) + 1)):
                if (xx - point[0]) ** 2 + (yy - point[1]) ** 2 <= r * r:
                    painted[yy, xx] = True
    affected = binary_dilation(changed, iterations=7)
    biome = np.isin(grounds, [0x46, 0x48, 0x56, 0x57, 0xbd])
    unmarked = np.array([entry[4] == 0 for entry in dictionary])[indices]
    rounded = np.where(affected & unmarked & (road | biome), painted, road)
    added, removed = rounded & ~road, road & ~rounded
    old_labels, old_count = label(road)
    new_labels, new_count = label(rounded)
    overlap = np.unique(np.stack((old_labels[road & rounded], new_labels[road & rounded])), axis=1)
    if (old_count != new_count or overlap.shape[1] != old_count
            or len(np.unique(overlap[0])) != old_count or len(np.unique(overlap[1])) != new_count
            or label(~road)[1] != label(~rounded)[1]):
        raise ValueError("road rounding changed connected components, loops or river crossings")
    if not np.isin(grounds[added], [0x46, 0x48, 0x56, 0x57, 0xbd]).all():
        raise ValueError("rounded road would overwrite water, beach, mountains or a set-piece floor")
    if any(dictionary[int(i)][4] != 0 for i in indices[added]):
        raise ValueError("rounded road would overwrite a region marker")

    plain = {(entry[0], entry[3]): i for i, entry in enumerate(dictionary)
             if not entry[1] and not entry[2] and entry[4] == 0}
    nearest = distance_transform_edt(~biome, return_distances=False, return_indices=True)
    for y, x in zip(*np.where(removed)):
        entry = dictionary[int(indices[nearest[0, y, x], nearest[1, y, x]])]
        key = (entry[0], entry[3])
        if key[0] not in (0x46, 0x48, 0x56, 0x57, 0xbd) or key not in plain:
            raise ValueError(f"cannot restore underlying biome at road cell {x},{y}: {key}")
        indices[y, x] = plain[key]
    road_entries = [i for i, entry in enumerate(dictionary) if entry == (0xd0, "", "", 0, 0)]
    if len(road_entries) != 1:
        raise ValueError("expected one undecorated Road dictionary entry")
    indices[added] = road_entries[0]
    print(f"Rounded roads: moved_centers={changed.sum()} added={added.sum()} removed={removed.sum()} components={old_count}")
    return indices.tobytes()
