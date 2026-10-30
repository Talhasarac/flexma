using TMPro;
using UnityEngine;
using UnityEngine.UI;

namespace Flexma
{
    /// <summary>How a layer is sized along one axis, as in Flexma.</summary>
    public enum Sizing { Fixed, Hug, Fill }

    /// <summary>
    /// A layer imported from Flexma: its id, which a re-import uses to update it in place, and how auto layout
    /// sizes it. The importer adds it to every layer; add your own components next to it freely.
    /// On text that follows its content, it gives layouts the text's size the way Flexma measures it: the design's
    /// own box while the text is what the design says, otherwise TextMeshPro's measure rounded like Flexma's.
    /// </summary>
    [DisallowMultipleComponent]
    public class FlexmaNode : MonoBehaviour, ILayoutElement
    {
        [Tooltip("The Flexma layer id; a re-import updates this object instead of making a new one.")]
        public string id;

        [Tooltip("How a parent's auto layout sizes this layer horizontally.")]
        public Sizing horizontal;

        [Tooltip("How a parent's auto layout sizes this layer vertically.")]
        public Sizing vertical;

        [Tooltip("Placed by its anchors, not by its parent's auto layout.")]
        public bool absolute;

        [Tooltip("Text: the width follows the text (Flexma's auto width).")]
        public bool textHugsWidth;

        [Tooltip("Text: the height follows the text (Flexma's auto width or auto height).")]
        public bool textHugsHeight;

        [Tooltip("Text: the line height in pixels.")]
        public float lineHeight;

        [Tooltip("Text: what the design says, and the box Flexma measured for it.")]
        public string designText;

        public Vector2 designSize;

        TMP_Text text;

        TMP_Text Text => text != null ? text : (text = GetComponent<TMP_Text>());

        /// <summary>The width a text that follows its content takes, or -1.</summary>
        public float TextWidth()
        {
            var t = Text;
            if ((t == null) || !textHugsWidth)
            {
                return -1;
            }
            return t.text == designText ? designSize.x : Mathf.Ceil(t.preferredWidth - 0.01f);
        }

        /// <summary>The height a text that follows its content takes at its current width (lines × line height,
        /// like Flexma), or -1.</summary>
        public float TextHeight()
        {
            var t = Text;
            if ((t == null) || !textHugsHeight)
            {
                return -1;
            }
            float width = ((RectTransform)transform).rect.width;
            if ((t.text == designText) && (Mathf.Abs(width - designSize.x) < 0.5f))
            {
                return designSize.y;
            }
            float lh = lineHeight > 0 ? lineHeight : t.fontSize * 1.2f;
            int lines = Mathf.Max(1, Mathf.RoundToInt(t.preferredHeight / lh));
            return Mathf.Ceil(lines * lh - 0.01f);
        }

        // ILayoutElement: ahead of TextMeshPro's own (priority 0), so content size fitters use Flexma's measure.
        public void CalculateLayoutInputHorizontal() { }
        public void CalculateLayoutInputVertical() { }
        public float minWidth => -1;
        public float preferredWidth => TextWidth();
        public float flexibleWidth => -1;
        public float minHeight => -1;
        public float preferredHeight => TextHeight();
        public float flexibleHeight => -1;
        public int layoutPriority => 1;
    }
}
