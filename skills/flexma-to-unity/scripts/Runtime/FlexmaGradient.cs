using System.Collections.Generic;
using UnityEngine;
using UnityEngine.UI;

namespace Flexma
{
    /// <summary>
    /// A Flexma linear gradient drawn with vertex colors on the layer's Image (one without a sprite), so it needs no
    /// texture and stays smooth at any size. The rectangle is cut into bands between the stops, each band's corners
    /// colored by where they fall on the gradient. Colors multiply with the Image's color, like a sprite's would.
    /// </summary>
    [ExecuteAlways]
    public class FlexmaGradient : BaseMeshEffect
    {
        [Tooltip("Flexma's angle in degrees: 0 runs left to right, 90 top to bottom.")]
        public float angle;
        [Tooltip("Where each color sits along the gradient, 0 to 1, in order.")]
        public float[] positions = new float[0];
        public Color[] colors = new Color[0];

        public void Set(float angle, float[] positions, Color[] colors)
        {
            this.angle = angle;
            this.positions = positions;
            this.colors = colors;
            if (graphic) graphic.SetVerticesDirty();
        }

        static readonly List<Vector2> poly = new List<Vector2>(), next = new List<Vector2>();

        public override void ModifyMesh(VertexHelper vh)
        {
            if (!IsActive() || positions == null || colors == null || positions.Length == 0 || positions.Length != colors.Length) return;
            var r = graphic.rectTransform.rect;
            if (r.width <= 0 || r.height <= 0) return;
            float cos = Mathf.Cos(angle * Mathf.Deg2Rad), sin = Mathf.Sin(angle * Mathf.Deg2Rad);
            // Where a point falls on the gradient, measured as Flexma does: across the box, from its top left, y down.
            System.Func<Vector2, float> at = p => ((p.x - r.xMin) / r.width - 0.5f) * cos + ((r.yMax - p.y) / r.height - 0.5f) * sin + 0.5f;
            var corners = new[] { new Vector2(r.xMin, r.yMin), new Vector2(r.xMin, r.yMax), new Vector2(r.xMax, r.yMax), new Vector2(r.xMax, r.yMin) };
            float lo = float.MaxValue, hi = float.MinValue;
            foreach (var c in corners) { lo = Mathf.Min(lo, at(c)); hi = Mathf.Max(hi, at(c)); }
            var cuts = new List<float> { lo };
            foreach (float t in positions) if (t > lo && t < hi) cuts.Add(t);
            cuts.Add(hi);
            Color tint = graphic.color;
            vh.Clear();
            for (int k = 0; k + 1 < cuts.Count; k++)
            {
                float a = cuts[k], b = cuts[k + 1];
                if (b - a < 1e-6f) continue;
                poly.Clear();
                poly.AddRange(corners);
                Clip(poly, p => at(p) - a);
                Clip(poly, p => b - at(p));
                if (poly.Count < 3) continue;
                int first = vh.currentVertCount;
                foreach (var p in poly)
                {
                    var uv = new Vector2((p.x - r.xMin) / r.width, (p.y - r.yMin) / r.height);
                    vh.AddVert(p, ColorAt(at(p)) * tint, uv);
                }
                for (int i = 1; i + 1 < poly.Count; i++) vh.AddTriangle(first, first + i, first + i + 1);
            }
        }

        /// <summary>Keeps the part of convex polygon <paramref name="p"/> where <paramref name="side"/> isn't negative.</summary>
        static void Clip(List<Vector2> p, System.Func<Vector2, float> side)
        {
            next.Clear();
            for (int i = 0; i < p.Count; i++)
            {
                Vector2 u = p[i], v = p[(i + 1) % p.Count];
                float su = side(u), sv = side(v);
                if (su >= 0) next.Add(u);
                if ((su >= 0) != (sv >= 0)) next.Add(Vector2.Lerp(u, v, su / (su - sv)));
            }
            p.Clear();
            p.AddRange(next);
        }

        Color ColorAt(float t)
        {
            if (t <= positions[0]) return colors[0];
            int last = positions.Length - 1;
            if (t >= positions[last]) return colors[last];
            int k = 1;
            while (positions[k] < t) k++;
            float span = positions[k] - positions[k - 1];
            return Color.Lerp(colors[k - 1], colors[k], span > 0 ? (t - positions[k - 1]) / span : 1);
        }
    }
}
