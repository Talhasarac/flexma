using UnityEngine;
using UnityEngine.EventSystems;
using UnityEngine.UI;

namespace Flexma
{
    /// <summary>
    /// A scroll view as tall as its content but never taller than the room its column leaves it, so a list hugs its
    /// rows like the design and scrolls them once they don't fit. Put it on the scroll view (a Fixed-height layer, with
    /// a ScrollRect and RectMask2D) inside auto-layout frame <see cref="Frame"/>, itself a child of <see cref="Column"/>
    /// (a Fill-height frame). It sets its height while the layout measures, after the layers above it have measured.
    /// </summary>
    public class FlexmaScrollHug : UIBehaviour, ILayoutElement
    {
        public RectTransform Content, Frame, Column;
        float measuredColumn = -1;

        public void CalculateLayoutInputHorizontal() { }

        public void CalculateLayoutInputVertical()
        {
            if (!Content || !Frame || !Column) return;
            measuredColumn = Column.rect.height;
            float room = measuredColumn - Taken(Column, Frame) - Taken(Frame, transform);
            float height = Mathf.Max(0, Mathf.Min(LayoutUtility.GetPreferredHeight(Content), room));
            var rt = (RectTransform)transform;
            if (Mathf.Abs(rt.rect.height - height) > 0.5f) rt.SetSizeWithCurrentAnchors(RectTransform.Axis.Vertical, height);
        }

        // A column that changed size after this measured it (a resized window) lays out again.
        void LateUpdate()
        {
            if (Column && Mathf.Abs(Column.rect.height - measuredColumn) > 0.5f)
                LayoutRebuilder.MarkLayoutForRebuild((RectTransform)transform);
        }

        /// <summary>The height <paramref name="frame"/>'s padding, gaps and children other than
        /// <paramref name="except"/> take, measured like Flexma's auto layout does.</summary>
        static float Taken(RectTransform frame, Transform except)
        {
            var layout = frame.GetComponent<FlexmaLayout>();
            if (!layout) return 0;
            float used = layout.paddingTop + layout.paddingBottom;
            int n = 0;
            for (int i = 0; i < frame.childCount; i++)
            {
                var c = frame.GetChild(i) as RectTransform;
                var node = c ? c.GetComponent<FlexmaNode>() : null;
                if (!node || node.absolute || !c.gameObject.activeSelf) continue;
                n++;
                if (c == except) continue;
                var l = c.GetComponent<FlexmaLayout>();
                float text = node.TextHeight();
                used += node.vertical == Sizing.Hug && l && l.isActiveAndEnabled ? l.preferredHeight : text >= 0 ? text : c.rect.height;
            }
            return used + layout.gap * Mathf.Max(0, n - 1);
        }

        public float minWidth => -1;
        public float preferredWidth => -1;
        public float flexibleWidth => -1;
        public float minHeight => -1;
        public float preferredHeight => -1;
        public float flexibleHeight => -1;
        public int layoutPriority => -1;
    }
}
