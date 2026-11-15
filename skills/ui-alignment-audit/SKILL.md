---
name: ui-alignment-audit
description: Audit the alignment of a UI from a screenshot (any PNG/JPG) the way a senior designer does a redline pass - detect the boxes, text and icons, draw guide lines and measurements over the image, and list near-miss alignments, uneven gaps, almost-equal sizes, off-centre content and lopsided padding with pixel values. Use whenever someone asks to check, audit, review, QA or "pixel-peep" the alignment, spacing, padding, grid or layout polish of a screen, mockup, Flexma/Figma frame, web page or game UI, asks "is this aligned / centred / evenly spaced?", wants guide lines or a ruler drawn on a screenshot, or wants a design checked before handoff - even if they only supply an image.
---

# UI alignment audit

Looking at a screenshot, you cannot see a 2px misalignment, and a downscaled image hides it
completely. This skill measures instead: `scripts/ui_align.py` finds the elements in the pixels,
compares their edges, and draws what it found so you can check each finding at high zoom before
you report it. It needs only an image - no design file, DOM or layer tree.

Requires Python 3 with `numpy`, `opencv-python-headless` and `pillow`
(`pip install numpy opencv-python-headless pillow`).

## Workflow

1. **Get a PNG at 1x.** One screen per image. For a design tool, export the frame at scale 1 so
   pixels equal design units (for a @2x capture pass `--scale 2`). In Flexma: `export_png` with
   the frame's `node_id` and `scale: 1`, then run the curl command it returns. `get_screenshot`
   is capped and resampled - do not measure on it.
2. **Run the audit.**
   ```bash
   python3 scripts/ui_align.py audit screen.png -o out/
   ```
   It prints the findings and writes four files to `out/`.
3. **Read `structure.png` first.** It shows what was detected: boxes in cyan, text/icon blocks in
   amber, a dashed green line wherever three or more siblings share an edge exactly, and magenta
   tags on evenly spaced runs (`10 x7` = seven 10px gaps). If obvious elements are missing or
   merged, fix detection (see Tuning) before trusting the findings. This image is also your map of
   the layout's real grid: look for edges that *should* be on a green line and are not.
4. **Check every finding in `zoom.png`.** Each tile is a 4x crop with the reference line in dashed
   green and the offending edge or measured inset in red. Confirm, or dismiss with a reason.
   `issues.png` shows where each numbered finding sits on the whole screen.
5. **Probe by hand where the audit cannot judge.** The audit only flags *near* misses (within the
   tolerance). Larger gaps - a grid that stops 50px short of the card below it, two panels whose
   first rows start at different heights - are design decisions it will not flag. Draw your own
   lines to test them:
   ```bash
   python3 scripts/ui_align.py guides screen.png --x 174,806 --y 222 --measure 756,300,806,300 --dim
   python3 scripts/ui_align.py guides screen.png --grid 8            # 8px grid
   python3 scripts/ui_align.py guides screen.png --columns 12,24,80  # 12 cols, 24 gutter, 80 margin
   python3 scripts/ui_align.py zoom   screen.png --at 1697,465 --size 100,60 --y 465
   ```
   `guides` also takes `--rect x,y,w,h`. Every output has pixel rulers, so read coordinates off
   one image and feed them to the next command.
6. **Report.** Lead with confirmed problems, most visible first. For each: what and where, the
   measured numbers, and the fix ("Craft button: right inset 12px, icon tile's left inset 8px -
   make both 8"). Group repeats ("all 4 fabricator rows"). List what you dismissed in one line so
   the reader knows it was looked at. Attach `issues.png` and `zoom.png`. If you have the design
   file, map each finding to its layer by position and name the layer - fix it only if asked.

## Reading the findings

| type | meaning |
|---|---|
| `alignment` | Edges or centres of siblings that almost share a line. The message gives the line most of them sit on and who is off, by how much. |
| `spacing` | A run of same-size boxes whose gaps are not all equal. |
| `size` | Sibling boxes whose widths or heights nearly match. |
| `centering` | A lone child that is nearly, but not exactly, centred in its box. |
| `padding` | A box whose content has slightly different insets on opposite sides. |

Elements are compared only with their siblings (same containing box), which is what keeps
unrelated parts of the screen from being matched against each other. IDs (`B12`, `C40`) are in
`report.json` with their x, y, w, h and parent.

**Severity is about measurement confidence, not importance.**
- `high` - both sides are boxes with crisp edges. The numbers are exact; the only question is
  whether the mismatch is intended.
- `low` - text or an icon is involved. Glyph bounds wobble by a pixel or two with the letters
  (side bearings, descenders), so always confirm in the zoom. Many of these are real - asymmetric
  padding usually shows up here - but expect some noise.

## Judgement

The tool reports coincidences as readily as mistakes. Before reporting a finding, ask:

- **Are these elements related?** A keycap whose right edge is 1px from a slot column three rows
  up is a coincidence. Two buttons in one row are not.
- **Is it systematic?** The same offset on every row is a spacing-token or padding decision (one
  fix); a single outlier is a nudged layer.
- **Is something overlapping?** A dragged item, tooltip or cursor drawn over a slot distorts that
  slot's measured box. Dismiss those.
- **Is it optical?** Icons and text are sometimes nudged on purpose. Text is centred by its line
  box, not its ink, so 1px vertical differences in text are normal. The audit already ignores
  text that only looks low because of descenders.
- **Would anyone see it?** 1px on a 600px panel edge that nothing else lines up with matters less
  than 2px between adjacent icons.

A clean result means "no near misses", not "well aligned": step 5 is where the bigger layout
questions get answered.

## Tuning

| flag | use when |
|---|---|
| `--tol N` (default 4) | Raise to catch larger offsets (more noise), lower for a strict pixel pass. |
| `--scale 2` | The screenshot is @2x/@3x. |
| `--roi x,y,w,h` | Audit one panel; cuts noise on busy screens. |
| `--edge N` (default 4) | Lower (2) if low-contrast cards are not detected; raise (8-12) on noisy or JPEG images. |
| `--min-box N` (default 14) | Lower to treat small chips/badges as boxes. |
| `--max-issues`, `--zoom-count`, `--zoom` | More or fewer findings in the images, bigger zoom tiles. |

## Limits

- Text and icons sitting directly on a photo, video or gradient backdrop are ignored (they cannot
  be separated from it); panels on top of the backdrop are still found. Use `guides` for those.
- Circles and odd shapes count as content, not boxes.
- Borderless fills are measured to within 1px and are held to a looser standard.
- JPEG artefacts create false edges - prefer PNG, or raise `--edge`.
- Text baselines are not measured; compare tops of same-height text, or zoom.
