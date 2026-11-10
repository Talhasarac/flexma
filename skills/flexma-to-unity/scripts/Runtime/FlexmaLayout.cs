using System.Collections.Generic;
using TMPro;
using UnityEngine;
using UnityEngine.EventSystems;
using UnityEngine.UI;

namespace Flexma
{
    public enum FlexmaLayoutMode { Horizontal, Vertical, Grid }

    public enum FlexmaAlign { Start, Center, End, SpaceBetween, Stretch }

    public enum FlexmaGridCorner { TopLeft, TopRight, BottomLeft, BottomRight }

    /// <summary>
    /// Flexma's auto layout on a uGUI RectTransform, with the same rules as the Flexma editor
    /// (flexma-doc layout.rs): padding, gap, alignment, hug and fill sizes, wrapping and grids. Children with
    /// a <see cref="FlexmaNode"/> take part; absolute ones keep their anchors. When text or children change,
    /// the frame reflows the way the design would.
    /// </summary>
    [ExecuteAlways, DisallowMultipleComponent, RequireComponent(typeof(RectTransform))]
    public class FlexmaLayout : UIBehaviour, ILayoutGroup, ILayoutElement
    {
        public FlexmaLayoutMode mode = FlexmaLayoutMode.Horizontal;
        public float paddingTop, paddingRight, paddingBottom, paddingLeft;
        [Tooltip("Space between children (between columns, for grids).")]
        public float gap;
        [Tooltip("Space between wrapped lines, or between grid rows.")]
        public float crossGap;
        public FlexmaAlign primary;
        public FlexmaAlign counter;
        [Tooltip("Wrap children onto more lines (needs a fixed size along the main axis).")]
        public bool wrap;
        [Tooltip("Grid: a fixed number of columns (0: not fixed).")]
        public int gridColumns;
        [Tooltip("Grid: a fixed number of rows (0: not fixed).")]
        public int gridRows;
        [Tooltip("Grid: every cell's size (zero: the largest child's).")]
        public Vector2 gridCell;
        public FlexmaGridCorner gridStartCorner;
        [Tooltip("Grid: fill columns first.")]
        public bool gridVertical;

        DrivenRectTransformTracker tracker;
        readonly List<RectTransform> flow = new List<RectTransform>();
        /// <summary>The size this frame hugs its content at.</summary>
        float hugWidth, hugHeight;

        RectTransform Rect => (RectTransform)transform;

        FlexmaNode Node => GetComponent<FlexmaNode>();

        // ---------------- ILayoutElement: a hugging frame's size, for ContentSizeFitter and parents ----------------
        public float minWidth => -1;
        public float preferredWidth => hugWidth;
        public float flexibleWidth => -1;
        public float minHeight => -1;
        public float preferredHeight => hugHeight;
        public float flexibleHeight => -1;
        public int layoutPriority => 1;

        public void CalculateLayoutInputHorizontal() => hugWidth = Measure(0);

        public void CalculateLayoutInputVertical() => hugHeight = Measure(1);

        public void SetLayoutHorizontal()
        {
            tracker.Clear();
            Arrange(0);
        }

        public void SetLayoutVertical() => Arrange(1);

        // ---------------- children ----------------
        List<RectTransform> Flow()
        {
            flow.Clear();
            for (int i = 0; i < transform.childCount; i++)
            {
                var c = transform.GetChild(i) as RectTransform;
                if ((c == null) || !c.gameObject.activeSelf)
                {
                    continue;
                }
                var n = c.GetComponent<FlexmaNode>();
                if ((n != null) && !n.absolute)
                {
                    flow.Add(c);
                }
            }
            return flow;
        }

        static Sizing SizingOf(RectTransform c, int axis)
        {
            var n = c.GetComponent<FlexmaNode>();
            return n == null ? Sizing.Fixed : (axis == 0 ? n.horizontal : n.vertical);
        }

        /// <summary>The size a child brings along `axis` before fill sizes are shared out: a hugging frame's
        /// content size, text's own size where it follows the text, otherwise its current size.</summary>
        static float SizeOf(RectTransform c, int axis)
        {
            var n = c.GetComponent<FlexmaNode>();
            if ((n != null) && (SizingOf(c, axis) == Sizing.Hug))
            {
                var l = c.GetComponent<FlexmaLayout>();
                if ((l != null) && l.isActiveAndEnabled)
                {
                    return axis == 0 ? l.hugWidth : l.hugHeight;
                }
            }
            if (n != null)
            {
                float s = axis == 0 ? n.TextWidth() : n.TextHeight();
                if (s >= 0)
                {
                    return s;
                }
            }
            return c.rect.size[axis];
        }

        static bool IsText(RectTransform c) => c.GetComponent<TMP_Text>() != null;

        float PadStart(int axis) => axis == 0 ? paddingLeft : paddingTop;

        float PadEnd(int axis) => axis == 0 ? paddingRight : paddingBottom;

        float Pads(int axis) => PadStart(axis) + PadEnd(axis);

        int MainAxis => mode == FlexmaLayoutMode.Vertical ? 1 : 0;

        bool Wraps => wrap && (mode != FlexmaLayoutMode.Grid) && (SizingOfSelf(MainAxis) != Sizing.Hug);

        Sizing SizingOfSelf(int axis)
        {
            var n = Node;
            return n == null ? Sizing.Fixed : (axis == 0 ? n.horizontal : n.vertical);
        }

        // ---------------- measuring (Flexma's measure_hug) ----------------
        float Measure(int axis)
        {
            var kids = Flow();
            if (mode == FlexmaLayoutMode.Grid)
            {
                var g = GridGeom(kids);
                return g.Block(axis) + Pads(axis);
            }
            int main = MainAxis, cross = 1 - main;
            if (Wraps)
            {
                if (axis == main)
                {
                    return Rect.rect.size[axis];
                }
                var lines = WrapLines(kids);
                float total = 0;
                foreach (var l in lines)
                {
                    total += l.Cross;
                }
                total += crossGap * Mathf.Max(0, lines.Count - 1);
                return total + Pads(axis);
            }
            if (axis == main)
            {
                float sum = 0;
                for (int i = 0; i < kids.Count; i++)
                {
                    sum += SizeOf(kids[i], main) + (i > 0 ? gap : 0);
                }
                return sum + Pads(axis);
            }
            // Children that fill the cross axis take the hugged size instead of driving it.
            float most = 0, mostAll = 0;
            foreach (var c in kids)
            {
                float s = SizeOf(c, cross);
                if (SizingOf(c, cross) != Sizing.Fill)
                {
                    most = Mathf.Max(most, s);
                }
                mostAll = Mathf.Max(mostAll, s);
            }
            return (most == 0 ? mostAll : most) + Pads(axis);
        }

        // ---------------- arranging (Flexma's arrange) ----------------
        void Arrange(int axis)
        {
            var kids = Flow();
            if (mode == FlexmaLayoutMode.Grid)
            {
                ArrangeGrid(kids, axis);
            }
            else if (Wraps)
            {
                ArrangeWrapped(kids, axis);
            }
            else
            {
                ArrangeLine(kids, axis);
            }
            // Absolute children that fill an axis span the whole frame along it.
            for (int i = 0; i < transform.childCount; i++)
            {
                var c = transform.GetChild(i) as RectTransform;
                var n = c != null ? c.GetComponent<FlexmaNode>() : null;
                if ((n != null) && n.absolute && (SizingOf(c, axis) == Sizing.Fill))
                {
                    Place(c, axis, 0, Rect.rect.size[axis]);
                }
            }
        }

        void ArrangeLine(List<RectTransform> kids, int axis)
        {
            int main = MainAxis;
            float inner = Mathf.Max(0, Rect.rect.size[axis] - Pads(axis));
            if (axis != main)
            {
                foreach (var c in kids)
                {
                    float cc = SizeOf(c, axis);
                    bool hugs = SizingOf(c, axis) == Sizing.Hug;
                    if ((SizingOf(c, axis) == Sizing.Fill) || ((counter == FlexmaAlign.Stretch) && hugs && !IsText(c)))
                    {
                        cc = inner;
                    }
                    Place(c, axis, PadStart(axis) + CrossOffset(inner, cc), cc);
                }
                return;
            }
            bool hugsMain = SizingOfSelf(main) == Sizing.Hug;
            float fixedSum = 0;
            int fills = 0;
            foreach (var c in kids)
            {
                if ((SizingOf(c, main) == Sizing.Fill) && !hugsMain)
                {
                    fills++;
                }
                else
                {
                    fixedSum += SizeOf(c, main);
                }
            }
            float gaps = kids.Count > 1 ? gap * (kids.Count - 1) : 0;
            float fillEach = fills > 0 ? Mathf.Max(0, (inner - fixedSum - gaps) / fills) : 0;
            var sizes = new float[kids.Count];
            float used = gaps;
            for (int i = 0; i < kids.Count; i++)
            {
                sizes[i] = ((SizingOf(kids[i], main) == Sizing.Fill) && !hugsMain) ? fillEach : SizeOf(kids[i], main);
                used += sizes[i];
            }
            float free = Mathf.Max(0, inner - used);
            float cursor = 0, step = gap;
            switch (primary)
            {
                case FlexmaAlign.Center: cursor = free / 2; break;
                case FlexmaAlign.End: cursor = free; break;
                case FlexmaAlign.SpaceBetween:
                    if (kids.Count > 1)
                    {
                        step = gap + free / (kids.Count - 1);
                    }
                    break;
            }
            for (int i = 0; i < kids.Count; i++)
            {
                Place(kids[i], main, PadStart(main) + cursor, sizes[i]);
                cursor += sizes[i] + step;
            }
        }

        float CrossOffset(float inner, float size)
        {
            switch (counter)
            {
                case FlexmaAlign.Center: return (inner - size) / 2;
                case FlexmaAlign.End: return inner - size;
                default: return 0;
            }
        }

        struct Line
        {
            public List<RectTransform> Items;
            public float Main, Cross;
        }

        List<Line> WrapLines(List<RectTransform> kids)
        {
            int main = MainAxis, cross = 1 - main;
            float inner = Rect.rect.size[main] - Pads(main);
            var lines = new List<Line>();
            foreach (var c in kids)
            {
                float cm = SizeOf(c, main);
                float cc = SizingOf(c, cross) == Sizing.Fill ? 0 : SizeOf(c, cross);
                if ((lines.Count > 0) && (lines[lines.Count - 1].Main + gap + cm <= inner + 0.01f))
                {
                    var l = lines[lines.Count - 1];
                    l.Main += gap + cm;
                    l.Cross = Mathf.Max(l.Cross, cc);
                    l.Items.Add(c);
                    lines[lines.Count - 1] = l;
                }
                else
                {
                    lines.Add(new Line { Items = new List<RectTransform> { c }, Main = cm, Cross = cc });
                }
            }
            return lines;
        }

        void ArrangeWrapped(List<RectTransform> kids, int axis)
        {
            int main = MainAxis, cross = 1 - main;
            float inner = Rect.rect.size[main] - Pads(main);
            float crossCursor = 0;
            foreach (var line in WrapLines(kids))
            {
                float free = Mathf.Max(0, inner - line.Main);
                float cursor = 0, step = gap;
                switch (primary)
                {
                    case FlexmaAlign.Center: cursor = free / 2; break;
                    case FlexmaAlign.End: cursor = free; break;
                    case FlexmaAlign.SpaceBetween:
                        if (line.Items.Count > 1)
                        {
                            step = gap + free / (line.Items.Count - 1);
                        }
                        break;
                }
                foreach (var c in line.Items)
                {
                    float cm = SizeOf(c, main);
                    if (axis == main)
                    {
                        Place(c, main, PadStart(main) + cursor, cm);
                    }
                    else
                    {
                        float cc = SizeOf(c, cross);
                        bool hugs = SizingOf(c, cross) == Sizing.Hug;
                        if ((SizingOf(c, cross) == Sizing.Fill) || ((counter == FlexmaAlign.Stretch) && hugs && !IsText(c)))
                        {
                            cc = line.Cross;
                        }
                        Place(c, cross, PadStart(cross) + crossCursor + CrossOffset(line.Cross, cc), cc);
                    }
                    cursor += cm + step;
                }
                crossCursor += line.Cross + crossGap;
            }
        }

        struct Grid
        {
            public int Cols, Rows;
            public Vector2 Cell, Gap;

            public float Block(int axis)
            {
                int k = axis == 0 ? Cols : Rows;
                return k == 0 ? 0 : k * Cell[axis] + (k - 1) * Gap[axis];
            }
        }

        /// <summary>Columns, rows and cell size, like Unity's GridLayoutGroup.</summary>
        Grid GridGeom(List<RectTransform> kids)
        {
            int count = kids.Count;
            var cell = gridCell;
            if ((cell.x <= 0) || (cell.y <= 0))
            {
                cell = Vector2.zero;
                foreach (var c in kids)
                {
                    cell = Vector2.Max(cell, c.rect.size);
                }
            }
            var gaps = new Vector2(gap, crossGap);
            int cols;
            if (gridColumns > 0)
            {
                cols = gridColumns;
            }
            else if (gridRows > 0)
            {
                cols = Mathf.Max(1, Mathf.CeilToInt(count / (float)gridRows));
            }
            else if (SizingOfSelf(0) == Sizing.Hug)
            {
                cols = Mathf.Max(1, Mathf.CeilToInt(Mathf.Sqrt(count)));
            }
            else
            {
                float inner = Rect.rect.width - paddingLeft - paddingRight;
                cols = Mathf.Max(1, Mathf.FloorToInt((inner + gaps.x) / Mathf.Max(1e-6f, cell.x + gaps.x)));
            }
            int rows = (gridColumns <= 0) && (gridRows > 0) ? Mathf.Max(gridRows, Mathf.CeilToInt(count / (float)cols)) : Mathf.CeilToInt(count / (float)cols);
            return new Grid { Cols = cols, Rows = rows, Cell = cell, Gap = gaps };
        }

        void ArrangeGrid(List<RectTransform> kids, int axis)
        {
            var g = GridGeom(kids);
            float inner = Rect.rect.size[axis] - Pads(axis);
            float free = inner - g.Block(axis);
            var align = axis == 0 ? primary : counter;
            float start = PadStart(axis) + (align == FlexmaAlign.Center ? free / 2 : align == FlexmaAlign.End ? free : 0);
            bool fixedCells = (gridCell.x > 0) && (gridCell.y > 0);
            for (int i = 0; i < kids.Count; i++)
            {
                int col = gridVertical ? i / Mathf.Max(1, g.Rows) : i % g.Cols;
                int row = gridVertical ? i % Mathf.Max(1, g.Rows) : i / g.Cols;
                if ((gridStartCorner == FlexmaGridCorner.TopRight) || (gridStartCorner == FlexmaGridCorner.BottomRight))
                {
                    col = Mathf.Max(0, g.Cols - 1 - col);
                }
                if ((gridStartCorner == FlexmaGridCorner.BottomLeft) || (gridStartCorner == FlexmaGridCorner.BottomRight))
                {
                    row = Mathf.Max(0, g.Rows - 1 - row);
                }
                int k = axis == 0 ? col : row;
                float size = fixedCells ? g.Cell[axis] : kids[i].rect.size[axis];
                Place(kids[i], axis, start + k * (g.Cell[axis] + g.Gap[axis]), size);
            }
        }

        /// <summary>Put a child at `pos` (from the left or top inner edge of this frame) with `size` along `axis`.
        /// A Fixed child's anchors and size stay its own, not driven: the editor drops driven values when it copies or
        /// saves an object, and a Fixed size has nothing to rebuild it from.</summary>
        void Place(RectTransform c, int axis, float pos, float size)
        {
            var n = c.GetComponent<FlexmaNode>();
            bool followsText = (n != null) && (axis == 0 ? n.textHugsWidth : n.textHugsHeight);
            bool sized = (SizingOf(c, axis) != Sizing.Fixed) || followsText || (Mathf.Abs(size - c.rect.size[axis]) > 0.01f);
            var driven = axis == 0 ? DrivenTransformProperties.AnchoredPositionX : DrivenTransformProperties.AnchoredPositionY;
            if (sized)
            {
                driven |= axis == 0
                    ? DrivenTransformProperties.AnchorMinX | DrivenTransformProperties.AnchorMaxX | DrivenTransformProperties.SizeDeltaX
                    : DrivenTransformProperties.AnchorMinY | DrivenTransformProperties.AnchorMaxY | DrivenTransformProperties.SizeDeltaY;
            }
            tracker.Add(this, c, driven);
            c.SetInsetAndSizeFromParentEdge(axis == 0 ? RectTransform.Edge.Left : RectTransform.Edge.Top, pos, Mathf.Max(0, size));
        }

        // ---------------- keeping it up to date (like LayoutGroup) ----------------
        protected override void OnEnable()
        {
            base.OnEnable();
            SetDirty();
        }

        protected override void OnDisable()
        {
            tracker.Clear();
            LayoutRebuilder.MarkLayoutForRebuild(Rect);
            base.OnDisable();
        }

        protected override void OnRectTransformDimensionsChange()
        {
            base.OnRectTransformDimensionsChange();
            if (isActiveAndEnabled)
            {
                SetDirty();
            }
        }

        protected virtual void OnTransformChildrenChanged() => SetDirty();

        protected override void OnDidApplyAnimationProperties() => SetDirty();

#if UNITY_EDITOR
        protected override void OnValidate()
        {
            base.OnValidate();
            SetDirty();
        }
#endif

        /// <summary>Lay the frame out again (Unity calls this when its children or their text change).</summary>
        public void SetDirty()
        {
            if (!IsActive())
            {
                return;
            }
            if (!CanvasUpdateRegistry.IsRebuildingLayout())
            {
                LayoutRebuilder.MarkLayoutForRebuild(Rect);
            }
        }
    }
}
