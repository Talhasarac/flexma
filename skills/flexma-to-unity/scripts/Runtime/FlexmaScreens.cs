using System;
using System.Collections.Generic;
using TMPro;
using UnityEngine;
using UnityEngine.Events;
using UnityEngine.UI;

namespace Flexma
{
    /// <summary>
    /// Game code's side of imported Flexma screens: load a frame's prefab, find its layers by name, fill in data, wire
    /// buttons and switch states, without restyling anything. A look changes in Flexma and comes back with the next
    /// import. States the game switches between (a selected tab, an affordable price) are copied from the layer the
    /// design shows in that state (<see cref="Look"/>).
    /// </summary>
    public static class FlexmaScreens
    {
        /// <summary>Where the screen prefabs are under a Resources folder: the importer's Settings.Folder (inside
        /// Resources) + "/FILE_ID/Screens/". Set it once at startup.</summary>
        public static string Folder = "Flexma/";

        const string PaintName = "<paint>";

        /// <summary>An instance of the prefab of Flexma frame <paramref name="frame"/> (its name, "/" written "-").</summary>
        public static Transform Load(string frame, Transform parent)
        {
            var go = UnityEngine.Object.Instantiate(Prefab(frame).gameObject, parent, false);
            go.name = frame;
            return go.transform;
        }

        /// <summary>The prefab of Flexma frame <paramref name="frame"/> itself, for reading samples (states, list items)
        /// without instantiating it.</summary>
        public static Transform Prefab(string frame)
        {
            var prefab = Resources.Load<GameObject>(Folder + frame);
            if (!prefab) throw new InvalidOperationException("Flexma screen '" + frame + "' isn't imported (looked in Resources/" + Folder + ")");
            return prefab.transform;
        }

        /// <summary>One layer of frame <paramref name="frame"/>, moved under <paramref name="parent"/>; the rest of the
        /// frame is destroyed at once (so it never draws over anything). For a screen made of pieces from several
        /// frames: one window's chrome with each tab's body taken from that tab's frame.</summary>
        public static Transform Take(string frame, string path, Transform parent)
        {
            var screen = Load(frame, parent);
            var layer = Find(screen, path);
            layer.SetParent(parent, false);
            UnityEngine.Object.DestroyImmediate(screen.gameObject);
            return layer;
        }

        /// <summary>The layer at <paramref name="path"/>: layer names separated by "/", each the first one with that name
        /// anywhere below the previous (nearest first). A layer whose own name has a "/" ("Toggle/On", a component
        /// variant) is found by its whole name first.</summary>
        public static Transform Find(Transform root, string path)
        {
            var t = TryFind(root, path);
            if (!t) throw new InvalidOperationException("No layer '" + path + "' in '" + root.name + "'");
            return t;
        }

        /// <summary>Like <see cref="Find"/>, or null.</summary>
        public static Transform TryFind(Transform root, string path)
        {
            var whole = Below(root, path);
            if (whole) return whole;
            var t = root;
            foreach (var name in path.Split('/'))
            {
                t = Below(t, name);
                if (!t) return null;
            }
            return t;
        }

        static readonly Queue<Transform> queue = new Queue<Transform>();

        static Transform Below(Transform root, string name)
        {
            queue.Clear();
            for (int i = 0; i < root.childCount; i++) queue.Enqueue(root.GetChild(i));
            while (queue.Count > 0)
            {
                var t = queue.Dequeue();
                if (t.name == name) { queue.Clear(); return t; }
                for (int i = 0; i < t.childCount; i++) queue.Enqueue(t.GetChild(i));
            }
            return null;
        }

        public static T Get<T>(Transform root, string path) where T : Component
        {
            var c = Find(root, path).GetComponent<T>();
            if (!c) throw new InvalidOperationException("Layer '" + path + "' in '" + root.name + "' has no " + typeof(T).Name);
            return c;
        }

        public static TMP_Text Text(Transform root, string path) => Get<TMP_Text>(root, path);

        /// <summary>The <paramref name="index"/>th layer under <paramref name="parent"/>, not counting the "&lt;paint&gt;"
        /// image. For layers named after their content (a text layer is named by its text).</summary>
        public static Transform Kid(Transform parent, int index)
        {
            int k = 0;
            for (int i = 0; i < parent.childCount; i++)
            {
                var c = parent.GetChild(i);
                if (c.name == PaintName) continue;
                if (k++ == index) return c;
            }
            throw new InvalidOperationException("'" + parent.name + "' has no layer " + index);
        }

        /// <summary>The image that draws a layer: on the layer itself or, when it reaches past its box (shadows), on its
        /// "&lt;paint&gt;" child. Null for a layer that draws nothing.</summary>
        public static Image Paint(Transform layer)
        {
            var img = layer.GetComponent<Image>();
            if (img) return img;
            var p = layer.Find(PaintName);
            return p ? p.GetComponent<Image>() : null;
        }

        /// <summary>Makes a layer clickable; hovering, pressing and focus dim what it draws a little. Pass
        /// <see cref="Navigation.Mode.None"/> when the game moves gamepad focus itself (Unity's would move it twice).</summary>
        public static Button Button(Transform layer, UnityAction onClick, Navigation.Mode navigation = Navigation.Mode.Automatic)
        {
            var b = layer.GetComponent<Button>();
            if (!b) b = layer.gameObject.AddComponent<Button>();
            // What the layer draws takes the tint; found before adding the invisible click target below.
            var paint = Paint(layer);
            // Something to click on where the layer draws nothing. A GameObject holds one Graphic, so text clicks on its own.
            Graphic hit = layer.GetComponent<Graphic>();
            if (!hit) { hit = layer.gameObject.AddComponent<Image>(); hit.color = Color.clear; }
            hit.raycastTarget = true;
            b.targetGraphic = paint && paint.sprite ? paint : hit;
            var nav = b.navigation;
            nav.mode = navigation;
            b.navigation = nav;
            b.transition = Selectable.Transition.ColorTint;
            var colors = ColorBlock.defaultColorBlock;
            colors.highlightedColor = new Color(0.86f, 0.86f, 0.86f);
            colors.selectedColor = new Color(0.86f, 0.86f, 0.86f);
            colors.pressedColor = new Color(0.72f, 0.72f, 0.72f);
            colors.fadeDuration = 0.06f;
            b.colors = colors;
            b.onClick.RemoveAllListeners();
            if (onClick != null) b.onClick.AddListener(onClick);
            return b;
        }

        public static void Show(GameObject go, bool on)
        {
            if (go.activeSelf != on) go.SetActive(on);
        }

        public static void Show(Component c, bool on) => Show(c.gameObject, on);

        /// <summary>Moves a design sample (a list row, a card) into a hidden holder under <paramref name="holder"/> and
        /// returns it, to <see cref="Clone"/> one copy per item.</summary>
        public static Transform Template(Transform sample, Transform holder)
        {
            var hidden = holder.Find("Templates");
            if (!hidden)
            {
                hidden = new GameObject("Templates", typeof(RectTransform)).transform;
                hidden.SetParent(holder, false);
                hidden.gameObject.SetActive(false);
            }
            sample.SetParent(hidden, false);
            return sample;
        }

        /// <summary>Destroys the design's sample layers under <paramref name="list"/> now. Destroy() would wait for the
        /// end of the frame (and never run in edit mode), and copies made meanwhile would copy them too.</summary>
        public static void ClearSamples(Transform list)
        {
            for (int i = list.childCount - 1; i >= 0; i--)
                if (list.GetChild(i).GetComponent<FlexmaNode>()) UnityEngine.Object.DestroyImmediate(list.GetChild(i).gameObject);
        }

        /// <summary>A copy of a design layer for a list item, under <paramref name="parent"/> (or next to it), shown.</summary>
        public static Transform Clone(Transform template, Transform parent = null)
        {
            var t = UnityEngine.Object.Instantiate(template, parent ? parent : template.parent, false);
            t.name = template.name;
            t.gameObject.SetActive(true);
            return t;
        }

        /// <summary>Shows <paramref name="sprite"/> in a design icon frame ("Icon: axe"), in the color the design gives
        /// its icon, for icons the data picks. Returns the image, to recolor it.</summary>
        public static Image SetIcon(Transform iconFrame, Sprite sprite)
        {
            var color = Color.white;
            for (int i = 0; i < iconFrame.childCount; i++)
            {
                var c = iconFrame.GetChild(i);
                var drawn = Paint(c);
                if (drawn && drawn.sprite) color = drawn.color;
                if (c.name != PaintName) c.gameObject.SetActive(false);
            }
            var img = iconFrame.GetComponent<Image>();
            if (!img) img = iconFrame.gameObject.AddComponent<Image>();
            img.sprite = sprite;
            img.type = Image.Type.Simple;
            img.preserveAspect = true;
            img.raycastTarget = false;
            if (img.color == Color.white || img.color == Color.clear) img.color = color;
            return img;
        }

        /// <summary>A design dim (black at some alpha) for a flat overlay made in code. In Linear color space Unity
        /// blends in linear light, which darkens less, so the alpha rises to darken about as much as the design, as
        /// imported dims already do.</summary>
        public static Color Dim(Color design)
        {
            if (QualitySettings.activeColorSpace != ColorSpace.Linear) return design;
            design.a = 1f - Mathf.Pow(1f - Mathf.Clamp01(design.a), 2.1f);
            return design;
        }

        /// <summary>
        /// How a layer looks in one state: the images it and its children draw (sprite, tint, slicing), their opacity
        /// and text colors, taken from a layer the design shows in that state. <see cref="Apply"/> puts it on another
        /// layer built the same way (the design's selected tab onto the tab being selected), matching children by
        /// their order. Icons and other shapes keep their own image and take only the state's color, so one tab's
        /// icon never replaces another's.
        /// </summary>
        public sealed class Look
        {
            struct Part
            {
                public int[] Path;
                public bool HasPaint, OnLayer, Shape;
                public Sprite Sprite;
                public Image.Type Type;
                public Color Color;
                public Vector2 PaintMin, PaintMax;
                public bool HasText;
                public Color TextColor;
                public float Alpha;
                public bool HasPosition;
                public Vector2 Position;
            }

            readonly List<Part> parts = new List<Part>();
            bool positions;

            /// <param name="withPositions">Also copy where children sit in layers auto layout doesn't place (a
            /// toggle's knob).</param>
            public static Look Of(Transform sample, bool withPositions = false)
            {
                var look = new Look { positions = withPositions };
                look.Capture(sample, new List<int>());
                return look;
            }

            static bool IsShape(Transform t)
            {
                var n = t.GetComponent<FlexmaNode>();
                return n && (n.type == "VECTOR" || n.type == "ELLIPSE" || n.type == "STAR" || n.type == "POLYGON" || n.type == "LINE");
            }

            void Capture(Transform t, List<int> path)
            {
                var part = new Part { Path = path.ToArray(), Shape = IsShape(t) };
                var group = t.GetComponent<CanvasGroup>();
                part.Alpha = group ? group.alpha : 1;
                if (positions && path.Count > 0 && t.parent && !t.parent.GetComponent<FlexmaLayout>())
                {
                    part.HasPosition = true;
                    part.Position = ((RectTransform)t).anchoredPosition;
                }
                var own = t.GetComponent<Image>();
                var child = t.Find(PaintName);
                var img = own && own.sprite ? own : child ? child.GetComponent<Image>() : null;
                if (img && img.sprite && img.enabled)
                {
                    part.HasPaint = true;
                    part.OnLayer = img == own;
                    part.Sprite = img.sprite;
                    part.Type = img.type;
                    part.Color = img.color;
                    if (!part.OnLayer)
                    {
                        part.PaintMin = ((RectTransform)child).offsetMin;
                        part.PaintMax = ((RectTransform)child).offsetMax;
                    }
                }
                var text = t.GetComponent<TMP_Text>();
                if (text) { part.HasText = true; part.TextColor = text.color; }
                parts.Add(part);
                int k = 0;
                for (int i = 0; i < t.childCount; i++)
                {
                    var c = t.GetChild(i);
                    if (c.name == PaintName) continue;
                    path.Add(k++);
                    Capture(c, path);
                    path.RemoveAt(path.Count - 1);
                }
            }

            static Transform Child(Transform t, int index)
            {
                int k = 0;
                for (int i = 0; i < t.childCount; i++)
                {
                    var c = t.GetChild(i);
                    if (c.name == PaintName) continue;
                    if (k++ == index) return c;
                }
                return null;
            }

            public void Apply(Transform target)
            {
                foreach (var part in parts)
                {
                    var t = target;
                    foreach (int i in part.Path)
                    {
                        t = t ? Child(t, i) : null;
                    }
                    if (!t) continue;
                    var own = t.GetComponent<Image>();
                    var child = t.Find(PaintName);
                    var group = t.GetComponent<CanvasGroup>();
                    if (part.Alpha < 1 || group) (group ? group : t.gameObject.AddComponent<CanvasGroup>()).alpha = part.Alpha;
                    if (part.HasPosition) ((RectTransform)t).anchoredPosition = part.Position;
                    var drawn = own && own.sprite ? own : child ? child.GetComponent<Image>() : null;
                    if (part.Shape && part.HasPaint && drawn && drawn.sprite)
                    {
                        drawn.color = part.Color;
                    }
                    else if (part.HasPaint && part.OnLayer)
                    {
                        if (!own) own = t.gameObject.AddComponent<Image>();
                        Set(own, part);
                        if (child) Show(child.gameObject, false);
                    }
                    else if (part.HasPaint)
                    {
                        if (!child)
                        {
                            child = new GameObject(PaintName, typeof(RectTransform)).transform;
                            child.SetParent(t, false);
                            child.gameObject.layer = t.gameObject.layer;
                            child.gameObject.AddComponent<LayoutElement>().ignoreLayout = true;
                            child.gameObject.AddComponent<Image>().raycastTarget = false;
                        }
                        child.SetSiblingIndex(0);
                        Show(child.gameObject, true);
                        var rt = (RectTransform)child;
                        rt.anchorMin = Vector2.zero;
                        rt.anchorMax = Vector2.one;
                        rt.offsetMin = part.PaintMin;
                        rt.offsetMax = part.PaintMax;
                        Set(child.GetComponent<Image>(), part);
                        if (own) { own.sprite = null; own.color = Color.clear; }
                    }
                    else
                    {
                        // Draws nothing in this state; an image that catches clicks stays, invisible.
                        if (own) { own.sprite = null; own.color = Color.clear; }
                        if (child) Show(child.gameObject, false);
                    }
                    if (part.HasText)
                    {
                        var text = t.GetComponent<TMP_Text>();
                        if (text) text.color = part.TextColor;
                    }
                }
            }

            static void Set(Image img, Part part)
            {
                img.sprite = part.Sprite;
                img.type = part.Type;
                img.color = part.Color;
                img.fillCenter = true;
                img.pixelsPerUnitMultiplier = 1;
            }
        }
    }
}
