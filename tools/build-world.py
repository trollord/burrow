#!/usr/bin/env python3
"""Regenerate src/world.js from Natural Earth 1:110m admin-0 countries (public domain).

    python3 tools/build-world.py

Vendored rather than fetched at runtime: Burrow must work offline, and an app that phones out
for a map on launch is a poor look for something selling privacy.
"""
import json, urllib.request, pathlib

URL = ("https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/"
       "geojson/ne_110m_admin_0_countries.geojson")
OUT = pathlib.Path(__file__).resolve().parent.parent / "src" / "world.js"

# The globe draws ~320px across for 360 degrees — just under a pixel per degree. 0.1 degrees is
# a tenth of a pixel, so rounding there is invisible and collapses many redundant points.
PRECISION = 1
MIN_AREA_SQ_DEG = 1.2      # anything smaller renders sub-pixel


def rings_of(geom):
    if geom["type"] == "Polygon":
        return geom["coordinates"]
    return [r for poly in geom["coordinates"] for r in poly]


def simplify(ring):
    out, last = [], None
    for lon, lat in ring:
        p = (round(lon, PRECISION), round(lat, PRECISION))
        if p != last:
            out.append(p)
            last = p
    return out


def area(ring):
    s = 0.0
    for i in range(len(ring)):
        x1, y1 = ring[i]
        x2, y2 = ring[(i + 1) % len(ring)]
        s += x1 * y2 - x2 * y1
    return abs(s) / 2


def main():
    with urllib.request.urlopen(URL, timeout=60) as r:
        src = json.load(r)

    rings = []
    for f in src["features"]:
        for ring in rings_of(f["geometry"]):
            s = simplify(ring)
            if len(s) >= 4 and area(s) >= MIN_AREA_SQ_DEG:
                rings.append(s)

    pts = sum(len(r) for r in rings)
    body = ",\n".join(
        "[" + ",".join(f"{c:g}" for p in r for c in p) + "]" for r in rings
    )
    OUT.write_text(
        "// Vendored world geometry: Natural Earth 1:110m admin-0 countries, public domain.\n"
        "// https://www.naturalearthdata.com/  — see naturalearthdata.com/about/terms-of-use/\n"
        "//\n"
        "// Coordinates are rounded to 0.1 degrees and consecutive duplicates removed, because the\n"
        "// globe draws ~320px across for 360 degrees and anything finer is invisible. Rings\n"
        "// smaller than about 1.2 square degrees are dropped: they would render sub-pixel.\n"
        "//\n"
        "// Each ring is a flat [lon, lat, lon, lat, ...] array — flat arrays are both smaller as\n"
        "// source and faster to walk than nested pairs.\n"
        f"//\n// {len(rings)} rings, {pts} points. Regenerate with tools/build-world.py.\n\n"
        f"export const RINGS = [\n{body}\n];\n"
    )
    print(f"wrote {OUT} — {len(rings)} rings, {pts} points")


if __name__ == "__main__":
    main()
