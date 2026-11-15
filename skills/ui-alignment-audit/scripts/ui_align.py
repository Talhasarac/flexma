#!/usr/bin/env python3
"""ui_align.py - pixel alignment audit for UI screenshots (any PNG/JPG).

  audit  IMAGE [-o DIR]            find elements, draw guides, list near-miss alignments
  guides IMAGE [--x ..] [--y ..]   draw your own guide lines / grid / boxes with a ruler
  zoom   IMAGE --at X,Y            enlarge a small area with a pixel grid

Needs: numpy, opencv-python(-headless), pillow.
"""
import argparse
import json
import os
import sys
from collections import Counter, defaultdict

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont

GREEN, RED, MAG = (40, 230, 140), (255, 70, 90), (255, 90, 230)
CYAN, AMBER, WHITE, INK = (60, 200, 255), (255, 205, 60), (255, 255, 255), (12, 14, 18)
RULER = 26


def font(size):
    try:
        return ImageFont.load_default(size=size)
    except TypeError:
        return ImageFont.load_default()


def load(path):
    im = Image.open(path)
    if im.mode in ("RGBA", "LA", "P"):
        im = im.convert("RGBA")
        bg = Image.new("RGBA", im.size, (255, 255, 255, 255))
        im = Image.alpha_composite(bg, im)
    return im.convert("RGB")


# ----------------------------------------------------------------- elements

class El:
    def __init__(self, kind, l, t, r, b):
        self.kind, self.l, self.t, self.r, self.b = kind, int(l), int(t), int(r), int(b)
        self.id, self.parent, self.approx = "", None, False  # approx: borderless fill, bounds +-1px

    w = property(lambda s: s.r - s.l)
    h = property(lambda s: s.b - s.t)
    cx = property(lambda s: (s.l + s.r) / 2)
    cy = property(lambda s: (s.t + s.b) / 2)
    area = property(lambda s: s.w * s.h)
    soft = property(lambda s: s.kind == "content" or s.approx)

    def json(self):
        return {"id": self.id, "kind": self.kind, "x": self.l, "y": self.t, "w": self.w, "h": self.h,
                "parent": self.parent.id if self.parent else None}


def merge_rects(rects):
    """Join glyphs into words/lines/icons: close neighbours on one line, or touching pieces."""
    rects = [tuple(r) for r in rects]
    while len(rects) > 1:
        a = np.array(rects, dtype=float)
        l, t, r, b = a.T
        h = b - t
        gx = np.maximum(l[:, None], l[None]) - np.minimum(r[:, None], r[None])
        gy = np.maximum(t[:, None], t[None]) - np.minimum(b[:, None], b[None])
        hmin, hmax = np.minimum(h[:, None], h[None]), np.maximum(h[:, None], h[None])
        near = ((gx <= np.clip(0.5 * hmax, 3, 12)) & (-gy >= 0.4 * hmin)) | ((gx <= 2) & (gy <= 2))
        root = list(range(len(rects)))

        def find(i):
            while root[i] != i:
                root[i] = root[root[i]]
                i = root[i]
            return i

        for i, j in zip(*np.nonzero(np.triu(near, 1))):
            root[find(i)] = find(j)
        groups = defaultdict(list)
        for i in range(len(rects)):
            groups[find(i)].append(rects[i])
        if len(groups) == len(rects):
            break
        rects = [(min(g[0] for g in gs), min(g[1] for g in gs), max(g[2] for g in gs), max(g[3] for g in gs))
                 for gs in groups.values()]
    return rects


def detect(rgb, edge_t=4, min_box=14):
    """Find boxes (panels, buttons, slots) and content (text lines, icons) from pixels alone.

    A pixel is an edge when it differs from its left/upper neighbour. Flat regions between edges
    are fills; the holes they enclose are either boxes (rectangular) or glyphs (everything else).
    """
    a = rgb.astype(np.int16)
    H, W = a.shape[:2]
    e = np.zeros((H, W), bool)
    e[:, 1:] |= np.abs(a[:, 1:] - a[:, :-1]).max(2) > edge_t
    e[1:] |= np.abs(a[1:] - a[:-1]).max(2) > edge_t
    e[1:, 1:] |= np.abs(a[1:, 1:] - a[:-1, :-1]).max(2) > edge_t  # seals square corners
    flat = (~e).astype(np.uint8)

    n, labels = cv2.connectedComponents(flat, connectivity=8)
    gray, lab = a.mean(2).ravel(), labels.ravel()
    cnt = np.maximum(np.bincount(lab, minlength=n), 1)
    m1 = np.bincount(lab, weights=gray, minlength=n) / cnt
    std = np.sqrt(np.maximum(np.bincount(lab, weights=gray * gray, minlength=n) / cnt - m1 * m1, 0))

    contours, hier = cv2.findContours(flat, cv2.RETR_CCOMP, cv2.CHAIN_APPROX_SIMPLE)
    boxes, regions, glyphs, lines = [], [], [], []
    for c, hr in zip(contours, hier[0] if hier is not None else []):
        x, y, w, h = cv2.boundingRect(c)
        rectish = w > 1 and h > 1 and cv2.contourArea(c) / ((w - 1) * (h - 1)) >= 0.82
        if hr[3] >= 0:  # hole in a flat region = something drawn on it
            l, t, r, b = x + 1, y + 1, x + w - 2, y + h - 2
            if r <= l or b <= t:
                continue
            if rectish and min(r - l, b - t) >= min_box:
                kind = "image" if e[t:b, l:r].mean() > 0.45 else "box"
                boxes.append(El(kind, l, t, r, b))
            elif min(r - l, b - t) <= 3 and max(r - l, b - t) >= 16:
                lines.append((l, t, r, b))
            elif max(r - l, b - t) >= 2:
                glyphs.append((l, t, r, b))
        elif rectish and min(w, h) >= min_box:  # a flat fill with no outline of its own
            p = c[0][0]
            regions.append((max(x - 1, 0), max(y - 1, 0), x + w, y + h, std[labels[p[1], p[0]]] <= 12))

    # an outline thicker than 1px yields two nested rectangles: keep the outer one
    boxes.sort(key=lambda b: -b.area)
    kept = []
    for b in boxes:
        if not any(0 <= b.l - k.l <= 3 and 0 <= b.t - k.t <= 3 and 0 <= k.r - b.r <= 3 and 0 <= k.b - b.b <= 3
                   for k in kept):
            kept.append(b)
    boxes = kept
    for l, t, r, b, is_flat in regions:
        dup = any(-1 <= l - k.l <= 8 and -1 <= t - k.t <= 8 and -1 <= k.r - r <= 8 and -1 <= k.b - b <= 8
                  for k in boxes)
        if not dup and is_flat:
            boxes.append(El("box", l, t, r, b))
            boxes[-1].approx = True

    def parents(rects, areas):
        if not boxes or not len(rects):
            return [None] * len(rects)
        B = np.array([(k.l, k.t, k.r, k.b, k.area) for k in boxes], dtype=float)
        R = np.array(rects, dtype=float)
        inside = ((B[None, :, 0] <= R[:, None, 0] + 1) & (B[None, :, 1] <= R[:, None, 1] + 1) &
                  (B[None, :, 2] >= R[:, None, 2] - 1) & (B[None, :, 3] >= R[:, None, 3] - 1) &
                  (B[None, :, 4] > np.asarray(areas, dtype=float)[:, None]))
        cost = np.where(inside, B[None, :, 4], np.inf)
        best = cost.argmin(1)
        return [boxes[j] if np.isfinite(cost[i, j]) else None for i, j in enumerate(best)]

    for b, p in zip(boxes, parents([(k.l, k.t, k.r, k.b) for k in boxes], [k.area for k in boxes])):
        b.parent = p

    els = list(boxes)
    by_parent = defaultdict(list)
    for g, p in zip(glyphs, parents(glyphs, [0] * len(glyphs))):
        if p is not None and p.kind == "box":  # content over photos/backdrops is noise
            by_parent[p].append(g)
    for p, gs in by_parent.items():
        for rect in merge_rects(gs):
            if max(rect[2] - rect[0], rect[3] - rect[1]) >= 4:
                c = El("content", *rect)
                c.parent = p
                els.append(c)
    for ln, p in zip(lines, parents(lines, [0] * len(lines))):
        if p is not None and p.kind == "box":
            c = El("line", *ln)
            c.parent = p
            els.append(c)

    els.sort(key=lambda k: (k.t, k.l))
    counts = Counter()
    for k in els:
        prefix = {"box": "B", "image": "B", "content": "C", "line": "L"}[k.kind]
        counts[prefix] += 1
        k.id = f"{prefix}{counts[prefix]}"
    return els


# ------------------------------------------------------------------- checks

EDGE = {
    "left": ("x", lambda k: k.l), "right": ("x", lambda k: k.r), "center-x": ("x", lambda k: k.cx),
    "top": ("y", lambda k: k.t), "bottom": ("y", lambda k: k.b), "center-y": ("y", lambda k: k.cy),
}


def clusters(items, tol):
    items = sorted(items, key=lambda p: p[0])
    out = []
    for it in items:
        if out and it[0] - out[-1][-1][0] <= tol:
            out[-1].append(it)
        else:
            out.append([it])
    return out


def span(els, axis):
    """Extent of a group along the direction a guide for `axis` runs."""
    return (min(k.t for k in els), max(k.b for k in els)) if axis == "x" else \
           (min(k.l for k in els), max(k.r for k in els))


def nearest(o, els):
    return min(els, key=lambda m: abs(m.cx - o.cx) + abs(m.cy - o.cy))


def clamp(v, lo, hi):
    return max(lo, min(hi, v))


class Audit:
    def __init__(self, els, tol):
        self.els, self.tol = els, tol
        self.issues, self.guides, self.runs = [], [], []
        self.seen, self.sized = set(), set()
        kids = defaultdict(list)
        for k in els:
            kids[k.parent].append(k)
        for parent, group in kids.items():
            boxes = [k for k in group if k.kind in ("box", "image")]
            content = [k for k in group if k.kind == "content"]
            exact = [k for k in boxes if not k.approx]
            self.sizes(exact)
            self.spacing(exact)
            self.align(boxes, list(EDGE), "high")
            if content:
                self.align(boxes + content, ["left", "right", "center-x", "center-y"], "low")
                for cl in clusters([(k.h, k) for k in content], 1):
                    self.align([k for _, k in cl], ["top", "bottom"], "low")
            if parent is not None and parent.kind == "box":
                self.centering(parent, boxes + content)
        order = {"high": 0, "low": 1}
        self.issues.sort(key=lambda i: (order[i["severity"]], -i["offset"]))
        for n, i in enumerate(self.issues, 1):
            i["n"] = n

    def add(self, **kw):
        self.issues.append(kw)

    def align(self, els, keys, severity):
        tol = self.tol
        for key in keys:
            axis, f = EDGE[key]
            for cl in clusters([(f(k), k) for k in els], tol):
                vals, members = [v for v, _ in cl], [k for _, k in cl]
                if len(cl) < 2:
                    continue
                if max(vals) == min(vals):
                    if len(cl) >= 3:
                        self.guides.append((axis, vals[0], members))
                    continue
                if key.startswith("center"):
                    sides = ("left", "right") if axis == "x" else ("top", "bottom")
                    if any(max(EDGE[s][1](k) for k in members) - min(EDGE[s][1](k) for k in members) <= tol
                           for s in sides):
                        continue  # they share an edge; the centres differ only because sizes do
                ref, n = Counter(vals).most_common(1)[0]
                soft = any(k.soft for k in members)
                if n == 1:
                    if soft:
                        continue  # two fuzzy edges that nearly meet prove nothing; need a pattern
                    ref = f(max(members, key=lambda k: k.area))
                good = [k for v, k in cl if v == ref]
                centre = EDGE["center-x" if axis == "x" else "center-y"][1]
                min_off = (3 if key == "center-y" else 2) if soft else 1
                dim = "w" if axis == "x" else "h"
                bad = [(v, k) for v, k in cl if abs(v - ref) >= min_off
                       and not (key not in ("left", "top") and (dim, k.id) in self.sized)
                       and not (not key.startswith("center") and  # different sizes, same centre: fine
                                any(abs(centre(k) - centre(m)) <= 0.5 for m in good))]
                if severity == "low" and not (any(k.soft for _, k in bad) or all(k.soft for k in good)):
                    continue  # box-vs-box was already judged by the strict pass
                if not bad:
                    continue
                bad.sort(key=lambda p: -abs(p[0] - ref))
                sig = (axis, frozenset(k.id for _, k in bad), round(bad[0][0] - ref))
                if sig in self.seen:
                    continue
                self.seen.add(sig)
                o = bad[0][1]
                m = nearest(o, good)
                a0, a1 = span(members, axis)
                o_dir = "v" if axis == "x" else "h"
                marks = [dict(o=o_dir, pos=ref, a0=a0, a1=a1, color=GREEN, dash=True)]
                for v, k in bad:
                    b0, b1 = span([k], axis)
                    marks.append(dict(o=o_dir, pos=v, a0=b0, a1=b1, color=RED))
                if axis == "x":
                    focus = [(bad[0][0], clamp(m.cy, o.t, o.b)), (ref, clamp(o.cy, m.t, m.b))]
                else:
                    focus = [(clamp(m.cx, o.l, o.r), bad[0][0]), (clamp(o.cx, m.l, m.r), ref)]
                offs = ", ".join(f"{k.id} at {axis}={v:g} ({v - ref:+g}px)" for v, k in bad)
                self.add(type="alignment", severity=severity, axis=axis, edge=key, ref=ref,
                         offset=abs(bad[0][0] - ref), ok=good, bad=[k for _, k in bad], marks=marks,
                         focus=focus, title=f"{key} {bad[0][0] - ref:+g}px",
                         message=f"{key} edges almost line up: {len(good)} element(s) at {axis}={ref:g}, but {offs}")

    def spacing(self, boxes):
        tol = self.tol
        for axis in "xy":
            centre, size = ("cy", "h") if axis == "x" else ("cx", "w")
            lo, hi = ("l", "r") if axis == "x" else ("t", "b")
            for c1 in clusters([(getattr(k, centre), k) for k in boxes], tol):
                for c2 in clusters([(getattr(k, size), k) for _, k in c1], tol):
                    row = sorted((k for _, k in c2), key=lambda k: getattr(k, lo))
                    if len(row) < 3:
                        continue
                    gaps = [getattr(b, lo) - getattr(a, hi) for a, b in zip(row, row[1:])]
                    if min(gaps) < 0:
                        continue
                    mode, n = Counter(gaps).most_common(1)[0]
                    odd = [i for i, g in enumerate(gaps) if 1 <= abs(g - mode) <= 2 * tol]
                    if n == 1 and len(gaps) > 2:
                        continue  # no rhythm to break
                    if not odd:
                        if len(set(gaps)) == 1:
                            self.runs.append((axis, row, mode))
                        continue
                    marks, bad = [], []
                    for i, g in enumerate(gaps):
                        if abs(g - mode) > 2 * tol:
                            continue
                        a, b = row[i], row[i + 1]
                        mid = (max(a.t, b.t) + min(a.b, b.b)) / 2 if axis == "x" else \
                              (max(a.l, b.l) + min(a.r, b.r)) / 2
                        marks.append(dict(o="h" if axis == "x" else "v", pos=mid, a0=getattr(a, hi),
                                          a1=getattr(b, lo), color=RED if i in odd else GREEN, label=f"{g}"))
                        if i in odd:
                            bad += [a, b]
                    i = max(odd, key=lambda i: abs(gaps[i] - mode))
                    a, b = row[i], row[i + 1]
                    g0 = (getattr(a, hi) + getattr(b, lo)) / 2
                    focus = [(g0, a.cy)] if axis == "x" else [(a.cx, g0)]
                    word = "horizontal" if axis == "x" else "vertical"
                    self.add(type="spacing", severity="high", axis=axis, ref=mode,
                             offset=abs(gaps[i] - mode), ok=[k for k in row if k not in bad], bad=bad,
                             marks=marks, focus=focus, title=f"gap {gaps[i]} vs {mode}px",
                             message=f"uneven {word} gaps in a run of {len(row)} boxes "
                                     f"({row[0].id}..{row[-1].id}): {gaps} - most are {mode}px")

    def sizes(self, boxes):
        tol = self.tol
        for dim, other, centre in (("w", "h", "cx"), ("h", "w", "cy")):
            for cl in clusters([(getattr(k, dim), k) for k in boxes], tol):
                vals = [v for v, _ in cl]
                if len(cl) < 2 or max(vals) == min(vals):
                    continue
                ref, n = Counter(vals).most_common(1)[0]
                if n == 1:
                    ref = max(vals)
                good = [k for v, k in cl if v == ref]
                bad = [k for v, k in cl if v != ref and any(
                    abs(getattr(k, other) - getattr(m, other)) <= tol or
                    (n > 1 and abs(getattr(k, centre) - getattr(m, centre)) <= tol) for m in good)]
                if not bad:
                    continue
                bad.sort(key=lambda k: -abs(getattr(k, dim) - ref))
                for k in bad:
                    self.sized.add((dim, k.id))
                o = bad[0]
                m = nearest(o, good)
                word = "width" if dim == "w" else "height"
                offs = ", ".join(f"{k.id} is {getattr(k, dim)}px" for k in bad)
                self.add(type="size", severity="high", axis="x" if dim == "w" else "y", ref=ref,
                         offset=abs(getattr(o, dim) - ref), ok=good, bad=bad, marks=[],
                         focus=[(o.r, o.b), (m.r, m.b)], title=f"{word} {getattr(o, dim)} vs {ref}px",
                         message=f"{word}s almost match: {len(good)} box(es) are {ref}px, but {offs}")

    def centering(self, p, kids):
        if not kids:
            return
        tol = self.tol
        ul, ut = min(k.l for k in kids), min(k.t for k in kids)
        ur, ub = max(k.r for k in kids), max(k.b for k in kids)
        soft = any(k.soft for k in kids) or p.approx
        single = len(kids) == 1
        for axis, a, b in (("x", ul - p.l, p.r - ur), ("y", ut - p.t, p.b - ub)):
            d = a - b
            least = (3 if axis == "y" else 2) if soft else 2
            if not least <= abs(d) <= (2 * tol if single and not soft else tol):
                continue
            if soft and axis == "y" and d > 0:
                continue  # text that looks low is usually just descenders (g, p, y) in its bounds
            names = ("left", "right") if axis == "x" else ("top", "bottom")
            pc = p.cx if axis == "x" else p.cy
            o_dir = "v" if axis == "x" else "h"
            p0, p1 = (p.t, p.b) if axis == "x" else (p.l, p.r)
            g_dir = "h" if axis == "x" else "v"   # the two insets, drawn as measured gaps
            cross = (ut + ub) / 2 if axis == "x" else (ul + ur) / 2
            e0, e1 = (p.l, p.r) if axis == "x" else (p.t, p.b)
            what = f"{kids[0].id} is {abs(d) / 2:g}px off-centre in {p.id}" if single else \
                   f"padding inside {p.id} is lopsided"
            self.add(type="centering" if single else "padding", severity="low" if soft else "high",
                     axis=axis, ref=pc, offset=abs(d) / 2, ok=[p], bad=kids if single else [],
                     marks=[dict(o=g_dir, pos=cross, a0=e0, a1=e0 + a, color=RED, label=f"{a}"),
                            dict(o=g_dir, pos=cross, a0=e1 - b, a1=e1, color=RED, label=f"{b}"),
                            dict(o=o_dir, pos=pc, a0=p0, a1=p1, color=GREEN, dash=True)],
                     focus=[(e0 + a / 2, cross), (e1 - b / 2, cross)] if axis == "x" else
                           [(cross, e0 + a / 2), (cross, e1 - b / 2)],
                     title=f"{names[0]} {a} / {names[1]} {b}px",
                     message=f"{what}: {names[0]} inset {a}px vs {names[1]} inset {b}px")


# ------------------------------------------------------------------ drawing

def dim(img, keep=0.45):
    g = img.convert("L").convert("RGB")
    return Image.blend(Image.blend(img, g, 0.5), Image.new("RGB", img.size, INK), 1 - keep)


def seg(d, o, pos, a0, a1, color, width=1, dash=False, s=1.0, off=(0, 0)):
    """Axis-aligned line. o='v' is a vertical line at x=pos from y=a0..a1; 'h' the other way."""
    pos, a0, a1 = pos * s, a0 * s, a1 * s
    step = (7, 5) if dash else (max(a1 - a0, 1), 0)
    a = a0
    while a < a1:
        e = min(a + step[0], a1)
        pts = [(pos + off[0], a + off[1]), (pos + off[0], e + off[1])] if o == "v" else \
              [(a + off[0], pos + off[1]), (e + off[0], pos + off[1])]
        d.line(pts, fill=color, width=width)
        a = e + step[1]


def label(d, xy, text, fg=WHITE, bg=INK, size=12, anchor="la", pad=2):
    f = font(size)
    box = d.textbbox(xy, text, font=f, anchor=anchor)
    d.rectangle((box[0] - pad, box[1] - pad, box[2] + pad, box[3] + pad), fill=bg)
    d.text(xy, text, font=f, fill=fg, anchor=anchor)


def draw_mark(d, m, s=1.0, off=(0, 0), width=1):
    seg(d, m["o"], m["pos"], m["a0"], m["a1"], m["color"], width, m.get("dash", False), s, off)
    if m.get("label"):
        mid = (m["a0"] + m["a1"]) / 2 * s
        cap = 4 * max(s / 2, 1)
        for a in (m["a0"], m["a1"]):  # end caps of a gap measurement
            seg(d, "v" if m["o"] == "h" else "h", a, m["pos"] - cap / s, m["pos"] + cap / s, m["color"], width, False, s, off)
        xy = (mid + off[0], m["pos"] * s + off[1] - cap - 2) if m["o"] == "h" else \
             (m["pos"] * s + off[0] + cap + 2, mid + off[1])
        label(d, xy, m["label"], fg=INK, bg=m["color"], size=11, anchor="mb" if m["o"] == "h" else "lm", pad=1)


def with_ruler(img, origin=(0, 0), step=None):
    """Add pixel rulers along the top and left so coordinates can be read off the picture."""
    W, H = img.size
    out = Image.new("RGB", (W + RULER, H + RULER), INK)
    out.paste(img, (RULER, RULER))
    d = ImageDraw.Draw(out)
    step = step or (100 if max(W, H) > 900 else 50 if max(W, H) > 400 else 10)
    minor = step // 10 if step >= 50 else step
    f = font(10)
    for horizontal, n, o in ((True, W, origin[0]), (False, H, origin[1])):
        first = -(-o // minor) * minor
        for v in range(int(first), int(o + n), minor):
            p = v - o + RULER
            ln = 10 if v % step == 0 else 6 if v % (step // 2 or 1) == 0 else 3
            d.line([(p, RULER - ln), (p, RULER)] if horizontal else [(RULER - ln, p), (RULER, p)], fill=(150, 160, 170))
            if v % step == 0:
                if horizontal:
                    d.text((p + 2, 1), str(v), font=f, fill=(200, 210, 220))
                else:
                    d.text((1, p + 1), str(v), font=f, fill=(200, 210, 220))
    return out


def draw_structure(img, audit):
    out = dim(img)
    d = ImageDraw.Draw(out)
    for k in audit.els:
        color = {"box": CYAN, "image": CYAN, "content": AMBER, "line": AMBER}[k.kind]
        d.rectangle((k.l, k.t, k.r - 1, k.b - 1), outline=tuple(int(c * 0.75) for c in color))
    done = set()
    for axis, v, members in audit.guides:
        a0, a1 = span(members, axis)
        key = (axis, v, a0, a1)
        if key in done:
            continue
        done.add(key)
        seg(d, "v" if axis == "x" else "h", v, a0 - 8, a1 + 8, GREEN, 1, True)
    for axis, row, gap in audit.runs:
        if gap <= 0:
            continue
        a, b = row[0], row[1]
        m = dict(o="h" if axis == "x" else "v", pos=a.cy if axis == "x" else a.cx,
                 a0=a.r if axis == "x" else a.b, a1=b.l if axis == "x" else b.t, color=MAG,
                 label=f"{gap} x{len(row) - 1}")
        draw_mark(d, m)
    return with_ruler(out)


def draw_issues(img, issues):
    out = dim(img)
    d = ImageDraw.Draw(out)
    for i in issues:
        for k in i["ok"]:
            d.rectangle((k.l, k.t, k.r - 1, k.b - 1), outline=(30, 150, 100))
    placed = []
    for i in issues:
        for k in i["bad"]:
            d.rectangle((k.l - 1, k.t - 1, k.r, k.b), outline=RED, width=2)
        for m in i["marks"]:
            draw_mark(d, m)
        x, y = i["focus"][0]
        x, y = x + 6, y - 20
        while any(abs(x - px) < 26 and abs(y - py) < 18 for px, py in placed):
            y += 18
        placed.append((x, y))
        color = RED if i["severity"] == "high" else AMBER
        label(d, (x, y), f"#{i['n']}", fg=INK, bg=color, size=13, pad=3)
    return with_ruler(out)


def zoom_tile(img, points, marks, factor, src=(96, 60)):
    """Magnified crops around `points` with the issue's lines drawn at exact pixel boundaries."""
    if len(points) == 2 and abs(points[0][0] - points[1][0]) < src[0] * 0.6 and \
            abs(points[0][1] - points[1][1]) < src[1] * 0.6:
        points = [((points[0][0] + points[1][0]) / 2, (points[0][1] + points[1][1]) / 2)]
    tiles = []
    for cx, cy in points:
        x0 = int(clamp(cx - src[0] / 2, 0, max(img.width - src[0], 0)))
        y0 = int(clamp(cy - src[1] / 2, 0, max(img.height - src[1], 0)))
        crop = img.crop((x0, y0, x0 + src[0], y0 + src[1])).resize((src[0] * factor, src[1] * factor), Image.NEAREST)
        d = ImageDraw.Draw(crop)
        for m in marks:
            lo, hi = (x0, x0 + src[0]) if m["o"] == "v" else (y0, y0 + src[1])
            if not lo <= m["pos"] <= hi:
                continue
            clipped = dict(m, a0=max(m["a0"], y0 if m["o"] == "v" else x0) - 0,
                           a1=min(m["a1"], (y0 + src[1]) if m["o"] == "v" else (x0 + src[0])))
            if clipped["a1"] <= clipped["a0"]:
                continue
            draw_mark(d, clipped, s=factor, off=(-x0 * factor, -y0 * factor), width=2)
        label(d, (3, 3), f"{x0},{y0}", size=10, fg=(200, 210, 220))
        tiles.append(crop)
    return tiles


def draw_zoom_sheet(img, issues, factor=4, cols=2):
    if not issues:
        return None
    src = (96, 60)
    tw, th, cap, gap = src[0] * factor * 2 + 6, src[1] * factor, 22, 10
    rows = -(-len(issues) // cols)
    sheet = Image.new("RGB", (cols * (tw + gap) + gap, rows * (th + cap + gap) + gap), INK)
    d = ImageDraw.Draw(sheet)
    for n, i in enumerate(issues):
        x, y = gap + (n % cols) * (tw + gap), gap + (n // cols) * (th + cap + gap)
        color = RED if i["severity"] == "high" else AMBER
        d.text((x, y + 3), f"#{i['n']}  {i['type']}: {i['title']}", font=font(14), fill=color)
        for j, t in enumerate(zoom_tile(img, i["focus"], i["marks"], factor, src)):
            sheet.paste(t, (x + j * (src[0] * factor + 6), y + cap))
    return sheet


# ----------------------------------------------------------------- commands

def ints(text):
    return [int(float(v)) for v in text.replace(";", ",").split(",") if v.strip()]


def cmd_audit(a):
    img = load(a.image)
    tol = max(1, round(a.tol * a.scale))
    ox = oy = 0
    work = img
    if a.roi:
        ox, oy, w, h = ints(a.roi)
        work = img.crop((ox, oy, ox + w, oy + h))
    els = detect(np.asarray(work), a.edge, max(4, round(a.min_box * a.scale)))
    for k in els:
        k.l, k.r, k.t, k.b = k.l + ox, k.r + ox, k.t + oy, k.b + oy
    audit = Audit(els, tol)
    shown = audit.issues[:a.max_issues]

    stem = os.path.splitext(os.path.basename(a.image))[0]
    out = a.out or os.path.join(os.path.dirname(os.path.abspath(a.image)), stem + "_audit")
    os.makedirs(out, exist_ok=True)
    paths = {"structure": os.path.join(out, "structure.png"), "issues": os.path.join(out, "issues.png"),
             "zoom": os.path.join(out, "zoom.png"), "report": os.path.join(out, "report.json")}
    draw_structure(img, audit).save(paths["structure"])
    draw_issues(img, shown).save(paths["issues"])
    sheet = draw_zoom_sheet(img, shown[:a.zoom_count], a.zoom)
    if sheet:
        sheet.save(paths["zoom"])
    else:
        paths.pop("zoom")
    report = {
        "image": os.path.abspath(a.image), "size": list(img.size), "tolerance_px": tol,
        "elements": [k.json() for k in els],
        "exact_alignments": len(audit.guides),
        "issues": [{"n": i["n"], "type": i["type"], "severity": i["severity"], "axis": i["axis"],
                    "offset_px": i["offset"], "message": i["message"],
                    "elements": [k.id for k in i["bad"]] or [k.id for k in i["ok"]]} for i in audit.issues],
    }
    with open(paths["report"], "w") as f:
        json.dump(report, f, indent=1)

    kinds = Counter(k.kind for k in els)
    high = sum(i["severity"] == "high" for i in audit.issues)
    print(f"{os.path.basename(a.image)}  {img.width}x{img.height}  tolerance {tol}px")
    print(f"elements: {kinds['box'] + kinds['image']} boxes, {kinds['content']} text/icon blocks, {kinds['line']} lines")
    print(f"exact alignments: {len(audit.guides)}   equal-gap runs: {len(audit.runs)}")
    print(f"issues: {len(audit.issues)}  ({high} high = box geometry, {len(audit.issues) - high} low = text/icon, verify by eye)")
    for i in shown:
        print(f"  #{i['n']:<3} {i['severity']:<4} {i['type']:<9} {i['message']}")
    if len(audit.issues) > len(shown):
        print(f"  ... {len(audit.issues) - len(shown)} more in report.json")
    for name, p in paths.items():
        print(f"{name}: {p}")


def cmd_guides(a):
    img = load(a.image)
    base = dim(img, 0.7) if a.dim else img.copy()
    d = ImageDraw.Draw(base)
    W, H = base.size
    if a.grid:
        for x in range(0, W, a.grid):
            d.line([(x, 0), (x, H)], fill=(255, 90, 230) if x % (a.grid * 4) == 0 else (120, 60, 110))
        for y in range(0, H, a.grid):
            d.line([(0, y), (W, y)], fill=(255, 90, 230) if y % (a.grid * 4) == 0 else (120, 60, 110))
    if a.columns:
        n, gutter, margin = ints(a.columns)
        cw = (W - 2 * margin - (n - 1) * gutter) / n
        ov = Image.new("RGBA", base.size, (0, 0, 0, 0))
        od = ImageDraw.Draw(ov)
        for i in range(n):
            x = margin + i * (cw + gutter)
            od.rectangle((x, 0, x + cw - 1, H), fill=(255, 70, 90, 46))
        base = Image.alpha_composite(base.convert("RGBA"), ov).convert("RGB")
        d = ImageDraw.Draw(base)
    for x in ints(a.x or ""):
        seg(d, "v", x, 0, H, CYAN)
        label(d, (x + 3, 3), f"x={x}", fg=INK, bg=CYAN, size=11)
    for y in ints(a.y or ""):
        seg(d, "h", y, 0, W, CYAN)
        label(d, (3, y + 3), f"y={y}", fg=INK, bg=CYAN, size=11)
    for r in a.rect or []:
        x, y, w, h = ints(r)
        d.rectangle((x, y, x + w - 1, y + h - 1), outline=RED)
        label(d, (x, y - 3), f"{x},{y} {w}x{h}", fg=INK, bg=RED, size=11, anchor="lb")
    for m in a.measure or []:
        x0, y0, x1, y1 = ints(m)
        if abs(x1 - x0) >= abs(y1 - y0):
            draw_mark(d, dict(o="h", pos=y0, a0=min(x0, x1), a1=max(x0, x1), color=MAG, label=f"{abs(x1 - x0)}"))
        else:
            draw_mark(d, dict(o="v", pos=x0, a0=min(y0, y1), a1=max(y0, y1), color=MAG, label=f"{abs(y1 - y0)}"))
    out = a.out or os.path.splitext(a.image)[0] + "_guides.png"
    with_ruler(base).save(out)
    print(out)


def cmd_zoom(a):
    img = load(a.image)
    cx, cy = ints(a.at)
    w, h = (ints(a.size) * 2)[:2]
    x0 = int(clamp(cx - w // 2, 0, max(img.width - w, 0)))
    y0 = int(clamp(cy - h // 2, 0, max(img.height - h, 0)))
    f = a.factor
    crop = img.crop((x0, y0, x0 + w, y0 + h)).resize((w * f, h * f), Image.NEAREST)
    d = ImageDraw.Draw(crop)
    if f >= 6:
        for i in range(0, w + 1):
            d.line([(i * f, 0), (i * f, h * f)], fill=(70, 80, 90) if (x0 + i) % 10 else (150, 160, 170))
        for j in range(0, h + 1):
            d.line([(0, j * f), (w * f, j * f)], fill=(70, 80, 90) if (y0 + j) % 10 else (150, 160, 170))
    for x in ints(a.x or ""):
        seg(d, "v", (x - x0) * f, 0, h * f, CYAN, 2)
    for y in ints(a.y or ""):
        seg(d, "h", (y - y0) * f, 0, w * f, CYAN, 2)
    ruled = Image.new("RGB", (crop.width + RULER, crop.height + RULER), INK)
    ruled.paste(crop, (RULER, RULER))
    d = ImageDraw.Draw(ruled)
    step = 10 if w <= 160 else 20
    for v in range(-(-x0 // step) * step, x0 + w, step):
        d.text(((v - x0) * f + RULER + 2, 2), str(v), font=font(10), fill=(200, 210, 220))
        d.line([((v - x0) * f + RULER, RULER - 8), ((v - x0) * f + RULER, RULER)], fill=(200, 210, 220))
    for v in range(-(-y0 // step) * step, y0 + h, step):
        d.text((1, (v - y0) * f + RULER + 1), str(v), font=font(10), fill=(200, 210, 220))
        d.line([(RULER - 8, (v - y0) * f + RULER), (RULER, (v - y0) * f + RULER)], fill=(200, 210, 220))
    out = a.out or f"{os.path.splitext(a.image)[0]}_zoom_{cx}_{cy}.png"
    ruled.save(out)
    print(out)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("audit", help="detect elements and report near-miss alignment, spacing, size, centring")
    s.add_argument("image")
    s.add_argument("-o", "--out", help="output folder (default: IMAGE_audit next to the image)")
    s.add_argument("--tol", type=float, default=4, help="how close counts as 'meant to match', in 1x px (default 4)")
    s.add_argument("--scale", type=float, default=1, help="2 for @2x screenshots; scales tolerance and minimum sizes")
    s.add_argument("--roi", help="x,y,w,h - audit only this area")
    s.add_argument("--edge", type=int, default=4, help="colour step that counts as an edge, 0-255 (default 4)")
    s.add_argument("--min-box", type=int, default=14, help="smallest side of a box in px (default 14)")
    s.add_argument("--max-issues", type=int, default=30)
    s.add_argument("--zoom", type=int, default=4, help="magnification of the zoom sheet")
    s.add_argument("--zoom-count", type=int, default=12, help="issues shown on the zoom sheet")
    s.set_defaults(fn=cmd_audit)

    s = sub.add_parser("guides", help="draw guide lines, a grid, columns, boxes or measurements")
    s.add_argument("image")
    s.add_argument("-o", "--out")
    s.add_argument("--x", help="vertical guides, e.g. 174,830")
    s.add_argument("--y", help="horizontal guides, e.g. 222,332")
    s.add_argument("--grid", type=int, help="square grid step in px, e.g. 8")
    s.add_argument("--columns", help="count,gutter,margin - layout columns, e.g. 12,24,80")
    s.add_argument("--rect", action="append", help="x,y,w,h outline (repeatable)")
    s.add_argument("--measure", action="append", help="x0,y0,x1,y1 distance marker (repeatable)")
    s.add_argument("--dim", action="store_true", help="darken the screenshot so lines stand out")
    s.set_defaults(fn=cmd_guides)

    s = sub.add_parser("zoom", help="magnify an area with a pixel grid")
    s.add_argument("image")
    s.add_argument("--at", required=True, help="centre x,y")
    s.add_argument("--size", default="120,80", help="source area w,h in px (default 120,80)")
    s.add_argument("--factor", type=int, default=8)
    s.add_argument("--x", help="vertical guides (image coordinates)")
    s.add_argument("--y", help="horizontal guides (image coordinates)")
    s.add_argument("-o", "--out")
    s.set_defaults(fn=cmd_zoom)

    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    sys.exit(main())
