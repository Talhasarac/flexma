#if UNITY_2022_3_OR_NEWER
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using System.Text.RegularExpressions;
using Newtonsoft.Json.Linq;
using UnityEditor;
using UnityEngine;
using UnityEngine.SceneManagement;
using UnityEngine.UIElements;

/// <summary>
/// The UI Toolkit half of FlexmaUIExporter (Options.UIToolkit): UIDocuments (UXML + USS) as editable Flexma layers.
///
/// Every document is rebuilt from its UXML in a throwaway panel in the export's temporary scene, with a copy of its
/// PanelSettings that draws into a temporary RenderTexture. Layout and styles are read back from that live panel
/// (resolvedStyle and the computed style), so the theme, the USS cascade and flexbox are Unity's own, and the scene,
/// its panels and their textures are never touched.
///   "Document / Default"  what the UXML shows when the scene starts
///   "Document / Panel"    an element hidden with display: none whose name or class a UI Toolkit script uses
/// Flexbox becomes Flexma auto layout wherever Flexma's layout rules reproduce Unity's computed rects; elsewhere the
/// children keep their computed positions. Repeated UXML templates (ui:Instance) become components, and a PNG of each
/// frame as Unity draws it is saved under renders/ for comparing with Flexma screenshots.
/// </summary>
public static partial class FlexmaUIExporter
{
    /// <summary>
    /// Called for every UI Toolkit frame after the document's copy is built (and the element the frame shows is switched
    /// on), before layout. The copy is rebuilt from its UXML for every frame, so change it freely: fill in runtime text,
    /// add list items, add the class a script would add. The scene's own documents are never touched.
    /// </summary>
    public static event Action<DocumentContext> PoseDocument;

    /// <summary>UIDocuments the project always leaves out, by GameObject name; add to it from the hooks.</summary>
    public static readonly List<string> SkippedDocumentNames = new List<string>();

    public sealed class DocumentContext
    {
        /// <summary>The original UIDocument in the scene (read only).</summary>
        public UIDocument Document;
        /// <summary>The export copy's root element (the UIDocument's rootVisualElement).</summary>
        public VisualElement Root;
        /// <summary>The element this frame shows, in the copy, or null for the document's default state.</summary>
        public VisualElement Screen;
        /// <summary>Set to a short phrase to note the pose in the frame's description and meta.</summary>
        public string Note;
    }

    private sealed partial class Exporter
    {
        const BindingFlags AnyMember = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static;
        static readonly Regex StringLiteral = new Regex("\"((?:[^\"\\\\\\r\\n]|\\\\.)*)\"");
        /// <summary>How far Flexma's auto layout may place a child from Unity's rect (Unity rounds layout to whole pixels).</summary>
        const float LayoutTolerance = 0.75f;

        /// <summary>A document rebuilt in the temporary scene, drawing into its own texture.</summary>
        sealed class DocCopy
        {
            public GameObject Go;
            public UIDocument Doc;
            public PanelSettings Settings;
            public RenderTexture Texture;
        }

        /// <summary>A child that flexbox places, as exported (Flexma px, relative to the parent layer).</summary>
        sealed class FlexKid
        {
            public JObject Node;
            public VisualElement Element;
            public float X, Y, W, H;
            public bool Visible = true;
            public bool FillMain, FillCross;
            /// <summary>Laid out as its margin box: a frame around the layer, padded by its margins (top, right, bottom, left).</summary>
            public float[] Margins;
        }

        /// <summary>Flexma auto layout props, along the main (layout direction) and cross axes.</summary>
        sealed class FlexPlan
        {
            public bool Horizontal, Wrap, HugMain, HugCross;
            public float PadMainStart, PadMainEnd, PadCrossStart, PadCrossEnd, Gap, RowGap;
            public string Justify = "START";
            public string Align = "START";
            public List<FlexKid> Order;
        }

        /// <summary>Border widths (Flexma px) and colors: top, right, bottom, left.</summary>
        sealed class Edges
        {
            public readonly float[] Widths = new float[4];
            public readonly Color[] Colors = new Color[4];

            public Edges(IResolvedStyle rs, float scale)
            {
                Widths[0] = rs.borderTopWidth * scale;
                Widths[1] = rs.borderRightWidth * scale;
                Widths[2] = rs.borderBottomWidth * scale;
                Widths[3] = rs.borderLeftWidth * scale;
                Colors[0] = rs.borderTopColor;
                Colors[1] = rs.borderRightColor;
                Colors[2] = rs.borderBottomColor;
                Colors[3] = rs.borderLeftColor;
            }

            public bool Side(int i)
            {
                return (Widths[i] > 0.01f) && (Colors[i].a > 0.004f);
            }

            public bool Any
            {
                get { return Side(0) || Side(1) || Side(2) || Side(3); }
            }

            /// <summary>Four equal sides: one Flexma stroke.</summary>
            public bool Uniform
            {
                get
                {
                    for (int i = 0; i < 4; i++)
                    {
                        if (!Side(i) || (Mathf.Abs(Widths[i] - Widths[0]) > 0.01f) || (Colors[i] != Colors[0]))
                        {
                            return false;
                        }
                    }
                    return true;
                }
            }
        }

        MethodInfo updatePanels;
        MethodInfo repaintPanels;
        MethodInfo renderPanels;
        MethodInfo lengthIsAuto;
        FieldInfo computedStyle;
        PropertyInfo richTextProperty;
        PropertyInfo templateSourceProperty;
        bool renderFailed;
        readonly Dictionary<string, PropertyInfo> computedProps = new Dictionary<string, PropertyInfo>();
        readonly Dictionary<UnityEngine.Object, float> lineHeights = new Dictionary<UnityEngine.Object, float>();
        /// <summary>String literals in the project's UI Toolkit scripts (element names, classes), and the scripts using them.</summary>
        readonly Dictionary<string, List<string>> uiLiterals = new Dictionary<string, List<string>>();
        readonly HashSet<JObject> spacers = new HashSet<JObject>(new SameObject());

        // The document being exported.
        UIDocument docOriginal;
        VisualElement docRoot;
        VisualElement docTree;
        string docId;
        string docPath;
        string docNote;
        float docSpritePPU;
        List<VisualElement> docScreenPath;

        partial void PrepareDocuments(ref bool ready, ref string why)
        {
            var assembly = typeof(VisualElement).Assembly;
            var utility = assembly.GetType("UnityEngine.UIElements.UIElementsRuntimeUtility");
            if (utility != null)
            {
                updatePanels = utility.GetMethod("UpdatePanels", AnyMember, null, Type.EmptyTypes, null);
                repaintPanels = utility.GetMethod("RepaintPanels", AnyMember, null, new[] { typeof(bool) }, null);
                renderPanels = utility.GetMethod("RenderOffscreenPanels", AnyMember, null, Type.EmptyTypes, null);
            }
            if (updatePanels == null)
            {
                why = "This Unity version has no UIElementsRuntimeUtility.UpdatePanels to lay out UI Toolkit panels in the editor";
                return;
            }
            // The computed style keeps USS keywords (auto, %) that resolvedStyle has already turned into pixels.
            computedStyle = typeof(VisualElement).GetField("m_Style", AnyMember);
            lengthIsAuto = typeof(Length).GetMethod("IsAuto", AnyMember, null, Type.EmptyTypes, null);
            if ((computedStyle == null) || (lengthIsAuto == null))
            {
                result.Warnings.Add("UI Toolkit: can't read computed styles in this Unity version; auto layout sizing and constraints are guessed from the layout");
            }
            richTextProperty = typeof(TextElement).GetProperty("enableRichText", AnyMember);
            templateSourceProperty = typeof(TemplateContainer).GetProperty("templateSource", AnyMember);
            ScanUIScripts();
            ready = true;
        }

        // ── documents ────────────────────────────────────────────

        partial void ExportDocuments(Scene scene, List<FrameEntry> page, float progressStart, float progressEnd)
        {
            var documents = new List<UIDocument>();
            foreach (var root in scene.GetRootGameObjects())
            {
                foreach (var d in root.GetComponentsInChildren<UIDocument>(true))
                {
                    // A document under another document is drawn inside it.
                    var parent = d.transform.parent;
                    if ((parent != null) && (parent.GetComponentInParent<UIDocument>(true) != null))
                    {
                        continue;
                    }
                    if (opt.SkipCanvasNames.Contains(d.name) || SkippedDocumentNames.Contains(d.name))
                    {
                        continue;
                    }
                    documents.Add(d);
                }
            }

            var groups = new HashSet<string>(page.Select(f => f.Canvas));
            var seen = new Dictionary<string, string>();
            for (int i = 0; i < documents.Count; i++)
            {
                var d = documents[i];
                if (Report(scene.name + ": " + d.name, progressStart + ((progressEnd - progressStart) * i / documents.Count)))
                {
                    throw new OperationCanceledException("Export cancelled");
                }
                string where = scene.name + " :: " + PathOf(d.transform);
                if (d.visualTreeAsset == null)
                {
                    result.Warnings.Add("UIDocument " + where + " has no Source Asset (its UI is built by script), so it was skipped");
                    continue;
                }
                if (d.panelSettings == null)
                {
                    result.Warnings.Add("UIDocument " + where + " has no Panel Settings, so it was skipped");
                    continue;
                }
                Transform switcher = null;
                bool shown = d.enabled && d.gameObject.activeInHierarchy;
                if (!shown)
                {
                    switcher = InactiveSwitch(d);
                    if (switcher == null)
                    {
                        excluded.Add(where + " (inactive UI document, nothing references it)");
                        continue;
                    }
                }
                // The same screen placed several times (a prefab in each level chunk) is exported once.
                string same = AssetDatabase.GetAssetPath(d.visualTreeAsset) + "|" + d.panelSettings.GetInstanceID() + "|" + shown;
                if (seen.TryGetValue(same, out var first))
                {
                    excluded.Add(where + " (same UXML and Panel Settings as " + first + ")");
                    continue;
                }
                seen[same] = PathOf(d.transform);
                string label = d.name;
                for (int dup = 2; groups.Contains(label); dup++)
                {
                    label = d.name + " (" + dup + ")";
                }
                groups.Add(label);
                ExportDocument(scene, d, label, switcher, page);
            }
        }

        /// <summary>The inactive object (or disabled UIDocument) a script switches on to show this document, if any.</summary>
        Transform InactiveSwitch(UIDocument d)
        {
            if (!d.enabled && IsReferenced(d.transform))
            {
                return d.transform;
            }
            for (var x = d.transform; x != null; x = x.parent)
            {
                if (!x.gameObject.activeSelf && IsReferenced(x))
                {
                    return x;
                }
            }
            return null;
        }

        void ExportDocument(Scene scene, UIDocument d, string label, Transform switcher, List<FrameEntry> page)
        {
            DocCopy copy = null;
            try
            {
                copy = CopyDocument(d);
                docOriginal = d;
                docPath = PathOf(d.transform);
                docId = GlobalObjectId.GetGlobalObjectIdSlow(d.gameObject).ToString();
                if (docId.StartsWith("GlobalObjectId_V1-0-"))
                {
                    docId = scene.path + "/" + docPath;
                }
                docSpritePPU = d.panelSettings.referenceSpritePixelsPerUnit;

                BuildCopy(copy, null);
                var size = docTree.layout.size;
                if (float.IsNaN(size.x) || (size.x < 1) || (size.y < 1))
                {
                    result.Warnings.Add("UIDocument " + scene.name + " :: " + docPath + " has no size after layout, so it was skipped");
                    return;
                }

                // Default first; then each hidden element a script shows gets its own frame.
                var screens = new List<int[]> { null };
                var names = new List<string> { label + " / Default" };
                EnumerateHidden(docRoot, 0, label, screens, names, scene.name);

                for (int fi = 0; fi < screens.Count; fi++)
                {
                    var screen = BuildCopy(copy, screens[fi]);
                    if ((screens[fi] != null) && (screen == null))
                    {
                        continue;
                    }
                    nodeCount = 0;
                    spacers.Clear();
                    var nodes = new JArray();
                    if (screen == null)
                    {
                        for (int i = 0; i < docRoot.hierarchy.childCount; i++)
                        {
                            Add(nodes, RenderElement(docRoot.hierarchy[i], Vector2.zero, 0, int.MaxValue));
                        }
                    }
                    else
                    {
                        Add(nodes, RenderElement(docScreenPath[0], Vector2.zero, 0, 0));
                    }
                    if (nodeCount == 0)
                    {
                        continue;
                    }
                    page.Add(new FrameEntry
                    {
                        Canvas = label, Name = names[fi], Description = DocumentDescription(scene, switcher, screen),
                        Width = size.x, Height = size.y, Nodes = nodes, Meta = DocumentMeta(scene, switcher, screen)
                    });
                    result.Nodes += nodeCount;
                    SaveRender(copy, scene.name, names[fi]);
                }
            }
            finally
            {
                DestroyCopy(copy);
                docOriginal = null;
                docRoot = null;
                docTree = null;
                docScreenPath = null;
            }
        }

        string DocumentDescription(Scene scene, Transform switcher, VisualElement screen)
        {
            string d = "Unity " + scene.name + " :: " + docPath + " (UI Toolkit, " + docOriginal.visualTreeAsset.name + ")";
            if (screen != null)
            {
                d += " :: " + ElementPath(screen);
            }
            if (!string.IsNullOrEmpty(docNote))
            {
                d += " (" + docNote + ")";
            }
            if (screen != null)
            {
                return d + " (display: none by default; shown by: " + UsedBy(screen) + ")";
            }
            if (switcher != null)
            {
                return d + " (document inactive by default; switched on by: " + ReferencedBy(switcher) + ")";
            }
            return d + " (default state, what the UXML shows when the scene starts)";
        }

        JObject DocumentMeta(Scene scene, Transform switcher, VisualElement screen)
        {
            var unity = new JObject
            {
                ["scene"] = scene.path, ["path"] = docPath, ["document"] = AssetDatabase.GetAssetPath(docOriginal.visualTreeAsset),
                ["panelSettings"] = AssetDatabase.GetAssetPath(docOriginal.panelSettings), ["state"] = "default"
            };
            if (switcher != null)
            {
                unity["state"] = "inactive-document";
                unity["switchedOnBy"] = ReferencedBy(switcher);
            }
            if (screen != null)
            {
                unity["state"] = "panel";
                unity["element"] = ElementPath(screen);
                unity["switchedOnBy"] = UsedBy(screen);
            }
            if (!string.IsNullOrEmpty(docNote))
            {
                unity["pose"] = docNote;
            }
            return new JObject { ["unity"] = unity };
        }

        // ── the throwaway panel ──────────────────────────────────

        DocCopy CopyDocument(UIDocument original)
        {
            var copy = new DocCopy();
            var settings = original.panelSettings;
            int w = Mathf.RoundToInt(opt.ScreenSize.x);
            int h = Mathf.RoundToInt(opt.ScreenSize.y);
            if (settings.targetTexture != null)
            {
                w = settings.targetTexture.width;
                h = settings.targetTexture.height;
            }
            copy.Texture = new RenderTexture(w, h, 24, RenderTextureFormat.ARGB32, RenderTextureReadWrite.sRGB) { name = "Flexma export", hideFlags = HideFlags.HideAndDontSave };
            copy.Texture.Create();
            copy.Settings = UnityEngine.Object.Instantiate(settings);
            copy.Settings.name = settings.name + " (Flexma export)";
            copy.Settings.hideFlags = HideFlags.HideAndDontSave;
            copy.Settings.targetTexture = copy.Texture;
            copy.Settings.clearColor = true;
            if (ColorUtility.TryParseHtmlString(opt.FrameBackground, out var background))
            {
                // The clear color is linear in a Linear-space project.
                if (QualitySettings.activeColorSpace == ColorSpace.Linear)
                {
                    background = background.linear;
                }
                copy.Settings.colorClearValue = background;
            }
            // Unity 6.2+ world-space panels are drawn flat into the texture like a screen.
            var renderMode = typeof(PanelSettings).GetProperty("renderMode", AnyMember);
            if ((renderMode != null) && renderMode.CanWrite && (Convert.ToInt32(renderMode.GetValue(copy.Settings)) != 0))
            {
                renderMode.SetValue(copy.Settings, Enum.ToObject(renderMode.PropertyType, 0));
                result.Warnings.Add("UIDocument " + PathOf(original.transform) + " is a world-space panel; it was exported flat at " + w + "×" + h);
            }

            copy.Go = new GameObject("Flexma export " + original.name) { hideFlags = HideFlags.DontSave };
            copy.Go.SetActive(false);
            copy.Doc = AddDocument(copy.Go, original, copy.Settings);
            // Documents nested under this one are drawn inside it.
            foreach (var child in original.GetComponentsInChildren<UIDocument>(true))
            {
                if ((child == original) || !child.enabled || !child.gameObject.activeSelf || (child.visualTreeAsset == null))
                {
                    continue;
                }
                var go = new GameObject(child.name) { hideFlags = HideFlags.DontSave };
                go.transform.SetParent(copy.Go.transform, false);
                AddDocument(go, child, copy.Settings);
            }
            copy.Go.SetActive(true);
            return copy;
        }

        static UIDocument AddDocument(GameObject go, UIDocument original, PanelSettings settings)
        {
            var doc = go.AddComponent<UIDocument>();
            doc.panelSettings = settings;
            doc.sortingOrder = original.sortingOrder;
            doc.visualTreeAsset = original.visualTreeAsset;
            return doc;
        }

        /// <summary>Rebuilds the copy from its UXML, switches on the element a frame shows, poses it and lays it out.</summary>
        VisualElement BuildCopy(DocCopy copy, int[] screen)
        {
            copy.Go.SetActive(false);
            copy.Go.SetActive(true);
            docRoot = copy.Doc.rootVisualElement;
            docTree = docRoot;
            if (docRoot.panel != null)
            {
                docTree = docRoot.panel.visualTree;
            }
            docScreenPath = null;
            docNote = null;
            UpdateUI();

            VisualElement shown = null;
            if (screen != null)
            {
                shown = docRoot;
                foreach (int i in screen)
                {
                    if ((shown == null) || (i >= shown.hierarchy.childCount))
                    {
                        return null;
                    }
                    shown = shown.hierarchy[i];
                }
                ShowChain(shown, null);
            }

            if (PoseDocument != null)
            {
                var context = new DocumentContext { Document = docOriginal, Root = docRoot, Screen = shown };
                try
                {
                    PoseDocument(context);
                }
                catch (Exception e)
                {
                    result.Warnings.Add("PoseDocument hook failed for " + docPath + ": " + e.Message);
                }
                docNote = context.Note;
            }
            UpdateUI();

            if (shown != null)
            {
                docScreenPath = new List<VisualElement>();
                for (var x = shown; (x != null) && (x != docRoot); x = x.hierarchy.parent)
                {
                    docScreenPath.Insert(0, x);
                }
            }
            return shown;
        }

        /// <summary>Shows an element and every ancestor hidden with display: none; remembers their inline values to put back.</summary>
        void ShowChain(VisualElement element, List<KeyValuePair<VisualElement, StyleEnum<DisplayStyle>>> before)
        {
            for (var x = element; (x != null) && (x != docRoot); x = x.hierarchy.parent)
            {
                if (x.resolvedStyle.display != DisplayStyle.None)
                {
                    continue;
                }
                if (before != null)
                {
                    before.Add(new KeyValuePair<VisualElement, StyleEnum<DisplayStyle>>(x, x.style.display));
                }
                x.style.display = DisplayStyle.Flex;
            }
        }

        void UpdateUI()
        {
            updatePanels.Invoke(null, null);
        }

        static void DestroyCopy(DocCopy copy)
        {
            if (copy == null)
            {
                return;
            }
            if (copy.Go != null)
            {
                UnityEngine.Object.DestroyImmediate(copy.Go);
            }
            if (copy.Settings != null)
            {
                UnityEngine.Object.DestroyImmediate(copy.Settings);
            }
            if (copy.Texture != null)
            {
                copy.Texture.Release();
                UnityEngine.Object.DestroyImmediate(copy.Texture);
            }
        }

        /// <summary>Draws the copy into its texture and saves it as renders/&lt;scene&gt;/&lt;frame&gt;.png.</summary>
        void SaveRender(DocCopy copy, string sceneName, string frameName)
        {
            if ((repaintPanels == null) || (renderPanels == null) || renderFailed)
            {
                return;
            }
            try
            {
                if (docScreenPath != null)
                {
                    // A panel frame shows only the panel and its ancestors; hide the rest without moving anything.
                    // The copy is rebuilt for the next frame.
                    foreach (var x in docScreenPath)
                    {
                        var parent = x.hierarchy.parent;
                        for (int i = 0; i < parent.hierarchy.childCount; i++)
                        {
                            if (parent.hierarchy[i] != x)
                            {
                                parent.hierarchy[i].style.visibility = Visibility.Hidden;
                            }
                        }
                    }
                    UpdateUI();
                }
                // Offscreen (texture) panels: update their draw lists, then draw.
                repaintPanels.Invoke(null, new object[] { true });
                renderPanels.Invoke(null, null);
                var previous = RenderTexture.active;
                RenderTexture.active = copy.Texture;
                var pixels = new Texture2D(copy.Texture.width, copy.Texture.height, TextureFormat.RGBA32, false);
                pixels.ReadPixels(new Rect(0, 0, copy.Texture.width, copy.Texture.height), 0, 0);
                pixels.Apply();
                RenderTexture.active = previous;
                string dir = Path.Combine(result.Folder, RendersFolder, SafeFileName(sceneName));
                Directory.CreateDirectory(dir);
                File.WriteAllBytes(Path.Combine(dir, SafeFileName(frameName) + ".png"), pixels.EncodeToPNG());
                UnityEngine.Object.DestroyImmediate(pixels);
            }
            catch (Exception e)
            {
                renderFailed = true;
                result.Warnings.Add("UI Toolkit: couldn't save Unity renders (" + e.GetBaseException().Message + ")");
            }
        }

        static string SafeFileName(string name)
        {
            foreach (char c in Path.GetInvalidFileNameChars())
            {
                name = name.Replace(c, '_');
            }
            return name;
        }

        // ── hidden panels ────────────────────────────────────────

        /// <summary>The string literals of every script in Assets/ that uses UI Toolkit, so Q("settings-panel") or
        /// RemoveFromClassList("hidden") marks those elements as shown at runtime.</summary>
        void ScanUIScripts()
        {
            uiLiterals.Clear();
            foreach (var guid in AssetDatabase.FindAssets("t:MonoScript", new[] { "Assets" }))
            {
                string path = AssetDatabase.GUIDToAssetPath(guid);
                if (!path.EndsWith(".cs", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }
                string text;
                try
                {
                    text = File.ReadAllText(path);
                }
                catch (IOException)
                {
                    continue;
                }
                if (!text.Contains("UIElements") && !text.Contains("UIDocument"))
                {
                    continue;
                }
                string script = Path.GetFileNameWithoutExtension(path);
                foreach (Match m in StringLiteral.Matches(text))
                {
                    string s = m.Groups[1].Value;
                    if ((s.Length == 0) || (s.Length > 80))
                    {
                        continue;
                    }
                    if (!uiLiterals.TryGetValue(s, out var list))
                    {
                        list = new List<string>();
                        uiLiterals[s] = list;
                    }
                    if (!list.Contains(script))
                    {
                        list.Add(script);
                    }
                }
            }
        }

        /// <summary>The element's name and USS classes, leaving out Unity's own (unity-*).</summary>
        static IEnumerable<string> Handles(VisualElement ve)
        {
            if (!string.IsNullOrEmpty(ve.name) && !ve.name.StartsWith("unity-"))
            {
                yield return ve.name;
            }
            foreach (var c in ve.GetClasses())
            {
                if (!c.StartsWith("unity-"))
                {
                    yield return c;
                }
            }
        }

        bool UsedElement(VisualElement ve)
        {
            return Handles(ve).Any(h => uiLiterals.ContainsKey(h));
        }

        string UsedBy(VisualElement ve)
        {
            return string.Join(", ", Handles(ve).Where(h => uiLiterals.ContainsKey(h)).SelectMany(h => uiLiterals[h]).Distinct().Take(4));
        }

        /// <summary>Like the uGUI Enumerate: every used hidden element whose ancestors are shown gets a frame, and inside
        /// those, used hidden elements that are panel-sized.</summary>
        void EnumerateHidden(VisualElement parent, int depth, string name, List<int[]> screens, List<string> names, string sceneName)
        {
            for (int i = 0; i < parent.hierarchy.childCount; i++)
            {
                var ch = parent.hierarchy[i];
                if (ch.resolvedStyle.display != DisplayStyle.None)
                {
                    EnumerateHidden(ch, depth, name, screens, names, sceneName);
                    continue;
                }
                if (!UsedElement(ch))
                {
                    // Parts of built-in controls (a Slider's dragger border) aren't the project's.
                    if (Handles(ch).Any())
                    {
                        excluded.Add(sceneName + " :: " + docPath + " :: " + ElementPath(ch) + " (display: none, no UI Toolkit script names it)");
                    }
                    continue;
                }
                if ((depth == 0) || BigElement(ch))
                {
                    string n = name + " / " + LayerName(ch);
                    string unique = n;
                    for (int dup = 2; names.Contains(unique); dup++)
                    {
                        unique = n + " (" + dup + ")";
                    }
                    screens.Add(IndexPath(ch));
                    names.Add(unique);
                    EnumerateHidden(ch, depth + 1, unique, screens, names, sceneName);
                }
                else
                {
                    EnumerateHidden(ch, depth, name, screens, names, sceneName);
                }
            }
        }

        /// <summary>Panel-sized: 15+ elements, or at least 15% of the panel once shown.</summary>
        bool BigElement(VisualElement ve)
        {
            int count = 0;
            ve.Query<VisualElement>().ForEach(_ => count++);
            if (count >= 15)
            {
                return true;
            }
            var before = new List<KeyValuePair<VisualElement, StyleEnum<DisplayStyle>>>();
            ShowChain(ve, before);
            UpdateUI();
            bool big = false;
            var box = LocalBox(ve);
            if (!float.IsNaN(box.width))
            {
                ElementGeo(ve, box, out _, out var u, out var v, out _);
                var panel = docTree.layout.size;
                big = Mathf.Abs((u.x * v.y) - (u.y * v.x)) >= (0.15f * panel.x * panel.y);
            }
            foreach (var kv in before)
            {
                kv.Key.style.display = kv.Value;
            }
            UpdateUI();
            return big;
        }

        int[] IndexPath(VisualElement ve)
        {
            var path = new List<int>();
            for (var x = ve; (x != null) && (x != docRoot); x = x.hierarchy.parent)
            {
                path.Insert(0, x.hierarchy.parent.hierarchy.IndexOf(x));
            }
            return path.ToArray();
        }

        // ── layers ───────────────────────────────────────────────

        /// <summary>
        /// One element as a Flexma layer: a text, a rectangle, or a frame holding what the element draws itself (border
        /// sides, image, text) and its children. pathIndex walks the ancestors of a panel frame, which show only the path.
        /// </summary>
        JObject RenderElement(VisualElement ve, Vector2 parentTopLeft, float parentAngle, int pathIndex)
        {
            var rs = ve.resolvedStyle;
            if (rs.display == DisplayStyle.None)
            {
                return null;
            }
            var box = LocalBox(ve);
            if (float.IsNaN(box.width) || float.IsNaN(box.height))
            {
                return null;
            }
            ElementGeo(ve, box, out var tl, out var u, out var v, out float angle);
            float scale = ScaleOf(box, u, v);

            bool onPath = (docScreenPath != null) && (pathIndex < docScreenPath.Count - 1);
            var kids = new List<KeyValuePair<VisualElement, JObject>>();
            if (onPath)
            {
                var next = docScreenPath[pathIndex + 1];
                kids.Add(new KeyValuePair<VisualElement, JObject>(next, RenderElement(next, tl, angle, pathIndex + 1)));
            }
            else
            {
                for (int i = 0; i < ve.hierarchy.childCount; i++)
                {
                    var ch = ve.hierarchy[i];
                    if (ch.resolvedStyle.display == DisplayStyle.None)
                    {
                        continue;
                    }
                    var node = RenderElement(ch, tl, angle, int.MaxValue);
                    if ((node == null) && InFlow(ch))
                    {
                        // An empty element still takes room in the flex flow; keep it while auto layout might need it.
                        node = Spacer(ch, tl, angle);
                    }
                    kids.Add(new KeyValuePair<VisualElement, JObject>(ch, node));
                }
            }
            bool hasKids = kids.Any(k => (k.Value != null) && !spacers.Contains(k.Value));

            string text = null;
            var te = ve as TextElement;
            if ((te != null) && (rs.color.a > 0.004f))
            {
                text = DisplayText(te);
            }
            bool hasText = !string.IsNullOrEmpty(text);
            string note = null;
            JObject content = null;
            if (ve is Image img)
            {
                content = ImagePaint(img, scale, ref note);
            }
            var fills = ElementFills(rs, scale, ref note);
            var edges = new Edges(rs, scale);
            bool hasBox = (fills.Count > 0) || edges.Any;
            string custom = CustomDrawing(ve);
            if (custom != null)
            {
                note = Join(note, custom);
            }

            var tail = new JObject();
            var constraints = ElementConstraints(ve);
            if (constraints != null)
            {
                tail["constraints"] = constraints;
            }
            var parent = ve.hierarchy.parent;
            if ((rs.visibility == Visibility.Hidden) && ((parent == null) || (parent.resolvedStyle.visibility != Visibility.Hidden)))
            {
                tail["visible"] = false;
            }
            if (rs.opacity < 0.999f)
            {
                tail["opacity"] = Math.Round(rs.opacity, 2);
            }

            if (!hasKids && (content == null))
            {
                if (hasText && !hasBox)
                {
                    var t = ElementText(ve, text, parentTopLeft, parentAngle, LayerName(ve));
                    t["key"] = ElementKey(ve);
                    Merge(t, tail);
                    if (note != null)
                    {
                        t["description"] = note;
                    }
                    t["meta"] = ElementMeta(ve);
                    return t;
                }
                if (!hasText && !hasBox && (custom == null))
                {
                    return null;
                }
                if (!hasText && hasBox && (edges.Uniform || !edges.Any))
                {
                    nodeCount++;
                    var r = new JObject { ["type"] = "rect", ["key"] = ElementKey(ve), ["name"] = LayerName(ve) };
                    Place(r, tl, u, v, angle, parentTopLeft, parentAngle);
                    Box(r, rs, fills, edges, scale);
                    Fixed(r);
                    Merge(r, tail);
                    if (note != null)
                    {
                        r["description"] = note;
                    }
                    r["meta"] = ElementMeta(ve);
                    return r;
                }
            }
            if (!hasKids && (content != null) && !hasText && !hasBox && SameRect(ve.contentRect, box))
            {
                nodeCount++;
                var r = new JObject { ["type"] = "rect", ["key"] = ElementKey(ve), ["name"] = LayerName(ve) };
                Place(r, tl, u, v, angle, parentTopLeft, parentAngle);
                r["fill"] = content;
                r["stroke"] = "none";
                r["radius"] = Radius(rs, scale);
                Fixed(r);
                Merge(r, tail);
                if (note != null)
                {
                    r["description"] = note;
                }
                r["meta"] = ElementMeta(ve);
                return r;
            }

            nodeCount++;
            var frame = new JObject { ["type"] = "frame", ["key"] = ElementKey(ve), ["name"] = LayerName(ve) };
            Place(frame, tl, u, v, angle, parentTopLeft, parentAngle);
            Box(frame, rs, fills, edges, scale);
            frame["clip"] = Overflows(ve);
            frame["layout"] = "NONE";
            Fixed(frame);

            // What the element draws itself, under its children: border sides, then its image or text.
            var own = new List<JObject>();
            if (edges.Any && !edges.Uniform)
            {
                own.AddRange(BorderSides(ve, frame, edges));
            }
            if (content != null)
            {
                nodeCount++;
                var r = new JObject { ["type"] = "rect", ["key"] = ElementKey(ve) + "#image", ["name"] = LayerName(ve) + " (image)" };
                ElementGeo(ve, ve.contentRect, out var ctl, out var cu, out var cv, out float cangle);
                Place(r, ctl, cu, cv, cangle, tl, angle);
                r["fill"] = content;
                r["stroke"] = "none";
                r["radius"] = 0;
                r["constraints"] = new JObject { ["horizontal"] = "STRETCH", ["vertical"] = "STRETCH" };
                Fixed(r);
                own.Add(r);
            }
            if (hasText)
            {
                var t = ElementText(ve, text, tl, angle, LayerName(ve) + " (text)");
                t["key"] = ElementKey(ve) + "#text";
                t["constraints"] = new JObject { ["horizontal"] = "STRETCH", ["vertical"] = "STRETCH" };
                own.Add(t);
            }

            string kept = null;
            var plan = onPath ? null : PlanLayout(ve, frame, scale, kids, out kept);
            if (plan == null)
            {
                kids.RemoveAll(k => (k.Value != null) && spacers.Contains(k.Value));
            }
            else
            {
                ApplyLayout(frame, plan, own, kids);
            }

            var children = new JArray();
            foreach (var n in own)
            {
                children.Add(n);
            }
            foreach (var k in kids)
            {
                Add(children, k.Value);
            }
            frame["children"] = children;
            Merge(frame, tail);
            if (note != null)
            {
                frame["description"] = note;
            }
            frame["meta"] = ElementMeta(ve);
            if (kept != null)
            {
                frame["meta"]["unity"]["layout"] = "kept Unity's positions: " + kept;
            }
            return frame;
        }

        /// <summary>Corners of a rect in the element's local space, in panel px (y down) relative to the frame.</summary>
        void ElementGeo(VisualElement ve, Rect local, out Vector2 topLeft, out Vector2 u, out Vector2 v, out float angle)
        {
            Vector2 P(Vector2 p)
            {
                return docTree.WorldToLocal(ve.LocalToWorld(p));
            }
            topLeft = P(new Vector2(local.xMin, local.yMin));
            u = P(new Vector2(local.xMax, local.yMin)) - topLeft;
            v = P(new Vector2(local.xMin, local.yMax)) - topLeft;
            var axis = P(new Vector2(local.xMin + 1, local.yMin)) - topLeft;
            angle = Mathf.Atan2(axis.y, axis.x) * Mathf.Rad2Deg;
        }

        /// <summary>The element's border box in its own space (VisualElement.rect is internal).</summary>
        static Rect LocalBox(VisualElement ve)
        {
            return new Rect(0, 0, ve.layout.width, ve.layout.height);
        }

        static bool SameRect(Rect a, Rect b)
        {
            return (Mathf.Abs(a.xMin - b.xMin) < 0.5f) && (Mathf.Abs(a.yMin - b.yMin) < 0.5f) && (Mathf.Abs(a.xMax - b.xMax) < 0.5f) && (Mathf.Abs(a.yMax - b.yMax) < 0.5f);
        }

        static bool InFlow(VisualElement ve)
        {
            return (ve.resolvedStyle.display != DisplayStyle.None) && (ve.resolvedStyle.position != Position.Absolute);
        }

        JObject Spacer(VisualElement ve, Vector2 parentTopLeft, float parentAngle)
        {
            var box = LocalBox(ve);
            if (float.IsNaN(box.width) || float.IsNaN(box.height))
            {
                return null;
            }
            ElementGeo(ve, box, out var tl, out var u, out var v, out float angle);
            var n = new JObject { ["type"] = "frame", ["key"] = ElementKey(ve), ["name"] = LayerName(ve) };
            Place(n, tl, u, v, angle, parentTopLeft, parentAngle);
            n["fill"] = "none";
            n["stroke"] = "none";
            n["radius"] = 0;
            n["clip"] = false;
            n["layout"] = "NONE";
            Fixed(n);
            if (ve.resolvedStyle.visibility == Visibility.Hidden)
            {
                n["visible"] = false;
            }
            n["meta"] = ElementMeta(ve);
            spacers.Add(n);
            return n;
        }

        /// <summary>Fill, stroke and corner radius. Every UI Toolkit layer states them, so instance overrides can undo a component's value.</summary>
        void Box(JObject n, IResolvedStyle rs, JArray fills, Edges edges, float scale)
        {
            if (fills.Count == 0)
            {
                n["fill"] = "none";
            }
            else if (fills.Count == 1)
            {
                n["fill"] = fills[0];
            }
            else
            {
                n["fill"] = fills;
            }
            if (edges.Uniform)
            {
                n["stroke"] = Hex(edges.Colors[0]);
                n["strokeWeight"] = Num(edges.Widths[0]);
                n["strokeAlign"] = "INSIDE";
            }
            else
            {
                n["stroke"] = "none";
            }
            n["radius"] = Radius(rs, scale);
        }

        static void Fixed(JObject n)
        {
            n["layoutSizingHorizontal"] = "FIXED";
            n["layoutSizingVertical"] = "FIXED";
        }

        static JToken Radius(IResolvedStyle rs, float scale)
        {
            float[] r = { rs.borderTopLeftRadius * scale, rs.borderTopRightRadius * scale, rs.borderBottomRightRadius * scale, rs.borderBottomLeftRadius * scale };
            if (r.All(x => Mathf.Abs(x - r[0]) < 0.05f))
            {
                return Num(r[0]);
            }
            return new JArray(r.Select(Num));
        }

        /// <summary>Borders with different widths or colors per side, as rectangles along the frame's edges.</summary>
        IEnumerable<JObject> BorderSides(VisualElement ve, JObject frame, Edges edges)
        {
            float w = (float)frame["w"];
            float h = (float)frame["h"];
            string[] names = { "top", "right", "bottom", "left" };
            for (int i = 0; i < 4; i++)
            {
                if (!edges.Side(i))
                {
                    continue;
                }
                float bw = edges.Widths[i];
                var rect = new[] { new Rect(0, 0, w, bw), new Rect(w - bw, 0, bw, h), new Rect(0, h - bw, w, bw), new Rect(0, 0, bw, h) }[i];
                string[] horizontal = { "STRETCH", "MAX", "STRETCH", "MIN" };
                string[] vertical = { "MIN", "STRETCH", "MAX", "STRETCH" };
                nodeCount++;
                var n = new JObject
                {
                    ["type"] = "rect", ["key"] = ElementKey(ve) + "#border-" + names[i], ["name"] = "border-" + names[i],
                    ["x"] = Num(rect.x), ["y"] = Num(rect.y), ["w"] = Num(Mathf.Max(rect.width, 1)), ["h"] = Num(Mathf.Max(rect.height, 1)),
                    ["fill"] = Hex(edges.Colors[i]), ["stroke"] = "none", ["radius"] = 0,
                    ["constraints"] = new JObject { ["horizontal"] = horizontal[i], ["vertical"] = vertical[i] }
                };
                Fixed(n);
                yield return n;
            }
        }

        bool Overflows(VisualElement ve)
        {
            var overflow = ComputedValue(ve, "overflow");
            return (overflow != null) && (overflow.ToString() == "Hidden");
        }

        static string Join(string a, string b)
        {
            if (string.IsNullOrEmpty(a))
            {
                return b;
            }
            return a + "; " + b;
        }

        /// <summary>Custom elements that draw with generateVisualContent (charts, gauges) show nothing in Flexma.</summary>
        static string CustomDrawing(VisualElement ve)
        {
            if ((ve.generateVisualContent == null) || (ve.GetType().Namespace == typeof(VisualElement).Namespace))
            {
                return null;
            }
            return "Unity draws this " + ve.GetType().Name + " with generateVisualContent, which isn't exported";
        }

        // ── paints ───────────────────────────────────────────────

        /// <summary>background-color, then background-image (sprite, texture, 9-slice, size, repeat, tint).</summary>
        JArray ElementFills(IResolvedStyle rs, float scale, ref string note)
        {
            var list = new JArray();
            if (rs.backgroundColor.a > 0.004f)
            {
                list.Add(Hex(rs.backgroundColor));
            }
            var bg = rs.backgroundImage;
            ImageEntry image = null;
            float ppu = 1f;
            Vector4 border = Vector4.zero;
            if (bg.sprite != null)
            {
                var sprite = bg.sprite;
                image = ExportImage(SourceTexture(sprite), sprite.rect, "sp" + sprite.GetInstanceID(), sprite.name);
                ppu = docSpritePPU / sprite.pixelsPerUnit;
                border = sprite.border;
            }
            else if (bg.texture != null)
            {
                var tex = bg.texture;
                image = ExportImage(tex, new Rect(0, 0, tex.width, tex.height), "tx" + tex.GetInstanceID(), tex.name);
            }
            else if (bg.renderTexture != null)
            {
                note = Join(note, "background is a runtime RenderTexture: " + bg.renderTexture.name);
                list.Add("#2b3240");
                return list;
            }
            else if (bg.vectorImage != null)
            {
                note = Join(note, "background is a vector image, which isn't exported: " + bg.vectorImage.name);
                return list;
            }
            if (image == null)
            {
                return list;
            }

            var p = new JObject { ["type"] = "IMAGE", ["asset"] = "@" + image.Key };
            var tint = rs.unityBackgroundImageTintColor;
            if (!IsWhite(tint))
            {
                p["tint"] = Hex(tint);
            }
            // -unity-slice-* (texture px) win over the sprite's own borders.
            if ((rs.unitySliceLeft > 0) || (rs.unitySliceTop > 0) || (rs.unitySliceRight > 0) || (rs.unitySliceBottom > 0))
            {
                border = new Vector4(rs.unitySliceLeft, rs.unitySliceBottom, rs.unitySliceRight, rs.unitySliceTop);
            }
            float k = image.Scale;
            if (border.sqrMagnitude > 0)
            {
                p["slice"] = new JObject
                {
                    ["top"] = Num(border.w * k), ["right"] = Num(border.z * k), ["bottom"] = Num(border.y * k), ["left"] = Num(border.x * k),
                    ["scale"] = Math.Round(scale * rs.unitySliceScale * ppu / k, 3)
                };
            }
            else if ((rs.backgroundRepeat.x == Repeat.Repeat) && (rs.backgroundRepeat.y == Repeat.Repeat))
            {
                p["fit"] = "TILE";
                p["tileScale"] = Math.Round(scale * ppu / k, 3);
            }
            else
            {
                var size = rs.backgroundSize;
                if (size.sizeType == BackgroundSizeType.Cover)
                {
                    p["fit"] = "FILL";
                }
                else if (size.sizeType == BackgroundSizeType.Contain)
                {
                    p["fit"] = "FIT";
                }
                else
                {
                    p["fit"] = "STRETCH";
                    bool full = (size.x.unit == LengthUnit.Percent) && (size.y.unit == LengthUnit.Percent) && (size.x.value > 99.9f) && (size.y.value > 99.9f);
                    if (!full)
                    {
                        note = Join(note, "background-size " + size.x + " " + size.y + " is shown stretched");
                    }
                }
            }
            list.Add(p);
            return list;
        }

        /// <summary>An Image element's sprite or texture, fitted like its scaleMode.</summary>
        JObject ImagePaint(Image img, float scale, ref string note)
        {
            ImageEntry image = null;
            if (img.sprite != null)
            {
                image = ExportImage(SourceTexture(img.sprite), img.sprite.rect, "sp" + img.sprite.GetInstanceID(), img.sprite.name);
            }
            else if (img.image is Texture2D tex)
            {
                var uv = img.uv;
                var region = new Rect(uv.x * tex.width, uv.y * tex.height, uv.width * tex.width, uv.height * tex.height);
                image = ExportImage(tex, region, "tx" + tex.GetInstanceID() + "_" + region, tex.name);
            }
            else if (img.image != null)
            {
                note = Join(note, "Image shows a runtime texture: " + img.image.name + " (" + img.image.GetType().Name + ")");
                return new JObject { ["type"] = "SOLID", ["color"] = "#2b3240" };
            }
            else if (img.vectorImage != null)
            {
                note = Join(note, "Image shows a vector image, which isn't exported: " + img.vectorImage.name);
            }
            if (image == null)
            {
                return null;
            }
            var p = new JObject { ["type"] = "IMAGE", ["asset"] = "@" + image.Key, ["fit"] = "FIT" };
            if (img.scaleMode == ScaleMode.ScaleAndCrop)
            {
                p["fit"] = "FILL";
            }
            else if (img.scaleMode == ScaleMode.StretchToFill)
            {
                p["fit"] = "STRETCH";
            }
            if (!IsWhite(img.tintColor))
            {
                p["tint"] = Hex(img.tintColor);
            }
            return p;
        }

        // ── text ─────────────────────────────────────────────────

        string DisplayText(TextElement te)
        {
            string text = te.text;
            if (string.IsNullOrEmpty(text))
            {
                return null;
            }
            bool rich = true;
            if (richTextProperty != null)
            {
                rich = (bool)richTextProperty.GetValue(te);
            }
            if (rich)
            {
                text = LineBreakTag.Replace(text, "\n");
                text = RichTag.Replace(text, "");
            }
            // UI Toolkit lays text out like TextMeshPro, which ignores one trailing line break.
            text = text.Replace("\r\n", "\n");
            if (text.EndsWith("\n"))
            {
                text = text.Substring(0, text.Length - 1);
            }
            if (text.Length == 0)
            {
                return null;
            }
            return text;
        }

        /// <summary>The element's text, in its content box (inside padding and border).</summary>
        JObject ElementText(VisualElement ve, string text, Vector2 parentTopLeft, float parentAngle, string name)
        {
            var rs = ve.resolvedStyle;
            ElementGeo(ve, ve.contentRect, out var tl, out var u, out var v, out float angle);
            ElementGeo(ve, LocalBox(ve), out _, out var u0, out var v0, out _);
            float scale = ScaleOf(LocalBox(ve), u0, v0);

            nodeCount++;
            var n = new JObject { ["type"] = "text", ["name"] = name };
            Place(n, tl, u, v, angle, parentTopLeft, parentAngle);
            n["text"] = text;
            float size = rs.fontSize;
            var auto = ComputedValue(ve, "unityTextAutoSize");
            if ((auto != null) && (Member(auto, "mode")?.ToString() == "BestFit"))
            {
                // Unity 6.2+ -unity-text-auto-size: like TMP, Flexma shrinks from the max size to fit the box.
                var min = Member(auto, "minSize") as Length?;
                var max = Member(auto, "maxSize") as Length?;
                if (min.HasValue && max.HasValue && (min.Value.unit == LengthUnit.Pixel) && (max.Value.unit == LengthUnit.Pixel))
                {
                    size = max.Value.value;
                    n["autoSize"] = new JArray(Num(min.Value.value * scale), Num(size * scale));
                }
            }
            n["size"] = Num(size * scale);

            var definition = rs.unityFontDefinition;
            UnityEngine.Object font = definition.fontAsset;
            if (font == null)
            {
                font = definition.font;
            }
            if (font == null)
            {
                font = rs.unityFont;
            }
            string fontKey = AssetFontKey(font);
            if (fontKey != null)
            {
                n["font"] = "@" + fontKey;
            }
            var style = rs.unityFontStyleAndWeight;
            if ((style == FontStyle.Bold) || (style == FontStyle.BoldAndItalic))
            {
                n["weight"] = 700;
            }
            if ((style == FontStyle.Italic) || (style == FontStyle.BoldAndItalic))
            {
                n["italic"] = true;
            }
            n["color"] = Hex(rs.color);
            int a = (int)rs.unityTextAlign;
            n["align"] = new[] { "LEFT", "CENTER", "RIGHT" }[a % 3];
            n["verticalAlign"] = new[] { "TOP", "CENTER", "BOTTOM" }[a / 3];
            n["lineHeight"] = Num(LineHeightRatio(font) * size * scale);
            if (Mathf.Abs(rs.letterSpacing * scale) > 0.05f)
            {
                n["letterSpacing"] = Num(rs.letterSpacing * scale);
            }
            n["textAutoResize"] = "NONE";
            if (opt.TextOverflow)
            {
                string white = rs.whiteSpace.ToString();
                if ((white == "NoWrap") || (white == "Pre"))
                {
                    n["wrap"] = false;
                }
                if (Overflows(ve))
                {
                    n["overflow"] = "CLIP";
                    if (rs.textOverflow == TextOverflow.Ellipsis)
                    {
                        n["overflow"] = "ELLIPSIS";
                    }
                }
            }
            if ((rs.unityTextOutlineWidth > 0.01f) && (rs.unityTextOutlineColor.a > 0.004f))
            {
                n["stroke"] = Hex(rs.unityTextOutlineColor);
                n["strokeWeight"] = Num(rs.unityTextOutlineWidth * scale);
                n["strokeAlign"] = "CENTER";
            }
            if (ComputedValue(ve, "textShadow") is TextShadow shadow && (shadow.color.a > 0.004f))
            {
                n["effects"] = new JArray(new JObject
                {
                    ["type"] = "DROP_SHADOW", ["x"] = Num(shadow.offset.x * scale), ["y"] = Num(shadow.offset.y * scale),
                    ["blur"] = Num(shadow.blurRadius * scale), ["spread"] = 0, ["color"] = Hex(shadow.color)
                });
            }
            Fixed(n);
            return n;
        }

        /// <summary>Line height / font size, from the font's face info (UI Toolkit text is TextCore, the TextMeshPro engine).</summary>
        float LineHeightRatio(UnityEngine.Object font)
        {
            if (font == null)
            {
                return 1.2f;
            }
            if (lineHeights.TryGetValue(font, out float ratio))
            {
                return ratio;
            }
            ratio = 0;
            if (font is Font f)
            {
                // UI Toolkit draws a Font through a TextCore font asset made from it.
                var assetType = Type.GetType("UnityEngine.TextCore.Text.FontAsset, UnityEngine.TextCoreTextEngineModule");
                var create = assetType?.GetMethod("CreateFontAsset", AnyMember, null, new[] { typeof(Font) }, null);
                if (create != null)
                {
                    try
                    {
                        // A new asset (with its own atlas and material) every call; nothing else uses it.
                        if (create.Invoke(null, new object[] { f }) is UnityEngine.Object made)
                        {
                            ratio = FaceRatio(made);
                            var owned = new List<UnityEngine.Object> { Member(made, "material") as UnityEngine.Object };
                            if (Member(made, "atlasTextures") is Texture2D[] atlases)
                            {
                                owned.AddRange(atlases);
                            }
                            foreach (var o in owned.Where(o => o != null))
                            {
                                UnityEngine.Object.DestroyImmediate(o);
                            }
                            UnityEngine.Object.DestroyImmediate(made);
                        }
                    }
                    catch (Exception)
                    {
                        ratio = 0;
                    }
                }
                if ((ratio <= 0) && (f.fontSize > 0) && (f.lineHeight > 0))
                {
                    ratio = f.lineHeight / (float)f.fontSize;
                }
            }
            else
            {
                ratio = FaceRatio(font);
            }
            if (ratio <= 0)
            {
                ratio = 1.2f;
            }
            lineHeights[font] = ratio;
            return ratio;
        }

        static float FaceRatio(UnityEngine.Object fontAsset)
        {
            var so = new SerializedObject(fontAsset);
            float lineHeight = Number(so.FindProperty("m_FaceInfo.m_LineHeight"));
            float pointSize = Number(so.FindProperty("m_FaceInfo.m_PointSize"));
            if ((lineHeight > 0) && (pointSize > 0))
            {
                return lineHeight / pointSize;
            }
            return 0;
        }

        static float Number(SerializedProperty p)
        {
            if (p == null)
            {
                return 0;
            }
            if (p.propertyType == SerializedPropertyType.Integer)
            {
                return p.intValue;
            }
            if (p.propertyType == SerializedPropertyType.Float)
            {
                return p.floatValue;
            }
            return 0;
        }

        /// <summary>A Font goes through the uGUI path; a TextCore font asset is read like a TMP one (same serialized layout).</summary>
        string AssetFontKey(UnityEngine.Object font)
        {
            if ((font == null) || (font is Font))
            {
                return FontKey(font);
            }
            if (fonts.TryGetValue(font, out var known))
            {
                return known.Key;
            }
            var entry = new FontEntry { Key = "F" + fonts.Count, Asset = font.name, Family = font.name, Style = "", Source = "" };
            var so = new SerializedObject(font);
            var family = so.FindProperty("m_FaceInfo.m_FamilyName");
            if ((family != null) && !string.IsNullOrEmpty(family.stringValue))
            {
                entry.Family = family.stringValue;
            }
            var styleName = so.FindProperty("m_FaceInfo.m_StyleName");
            if (styleName != null)
            {
                entry.Style = styleName.stringValue;
            }
            var source = so.FindProperty("m_SourceFontFile_EditorRef") ?? so.FindProperty("m_SourceFontFile");
            if ((source != null) && (source.objectReferenceValue != null))
            {
                entry.Source = AssetDatabase.GetAssetPath(source.objectReferenceValue);
            }
            var guid = so.FindProperty("m_CreationSettings.sourceFontFileGUID");
            if (string.IsNullOrEmpty(entry.Source) && (guid != null) && !string.IsNullOrEmpty(guid.stringValue))
            {
                entry.Source = AssetDatabase.GUIDToAssetPath(guid.stringValue);
            }
            if (!entry.Source.StartsWith("Assets/"))
            {
                entry.Source = "";
            }
            fonts[font] = entry;
            return entry.Key;
        }

        // ── flexbox -> auto layout ───────────────────────────────

        /// <summary>
        /// Flexma auto layout for a flex container, but only if Flexma's layout rules (crates/flexma-doc layout.rs) put
        /// every child where Unity did. Tried with FILL/HUG sizing first, then with fixed sizes; null keeps positions.
        /// </summary>
        FlexPlan PlanLayout(VisualElement ve, JObject frame, float scale, List<KeyValuePair<VisualElement, JObject>> kids, out string why)
        {
            why = null;
            if (!kids.Any(k => (k.Value != null) && InFlow(k.Key)))
            {
                return null;
            }
            // Children with margins that differ from their siblings' only line up as margin boxes, which cost a frame each.
            foreach (bool margins in new[] { false, true })
            {
                foreach (bool sizing in new[] { true, false })
                {
                    var plan = Plan(ve, frame, scale, FlowKids(kids), sizing, margins, out string reason);
                    if (plan != null)
                    {
                        return plan;
                    }
                    if (!sizing && (reason != null))
                    {
                        why = reason;
                    }
                }
            }
            return null;
        }

        static List<FlexKid> FlowKids(List<KeyValuePair<VisualElement, JObject>> kids)
        {
            var flow = new List<FlexKid>();
            foreach (var k in kids)
            {
                if ((k.Value == null) || !InFlow(k.Key))
                {
                    continue;
                }
                var n = k.Value;
                flow.Add(new FlexKid
                {
                    Node = n, Element = k.Key, X = (float)n["x"], Y = (float)n["y"], W = (float)n["w"], H = (float)n["h"],
                    Visible = (n["visible"] == null) || (bool)n["visible"]
                });
            }
            return flow;
        }

        FlexPlan Plan(VisualElement ve, JObject frame, float s, List<FlexKid> flow, bool sizing, bool margins, out string why)
        {
            why = null;
            var rs = ve.resolvedStyle;
            bool horizontal = (rs.flexDirection == FlexDirection.Row) || (rs.flexDirection == FlexDirection.RowReverse);
            bool reverse = (rs.flexDirection == FlexDirection.RowReverse) || (rs.flexDirection == FlexDirection.ColumnReverse);
            if (rs.flexWrap == Wrap.WrapReverse)
            {
                why = "flex-wrap: wrap-reverse";
                return null;
            }
            var order = new List<FlexKid>(flow);
            if (reverse)
            {
                order.Reverse();
            }
            if (order.Any(k => (k.Node["rotation"] != null) || (k.Node["flipY"] != null)))
            {
                why = "a child is rotated";
                return null;
            }
            float w = (float)frame["w"];
            float h = (float)frame["h"];
            var p = new FlexPlan { Horizontal = horizontal, Wrap = rs.flexWrap == Wrap.Wrap, Order = order };

            // Flexma padding = Unity's border + padding, plus the room between the outer children's margin boxes and
            // their exported boxes (their margins, and a text's own padding: Flexma texts have none).
            Rect Outer(FlexKid k)
            {
                var l = k.Element.layout;
                var m = k.Element.resolvedStyle;
                return Rect.MinMaxRect((l.xMin - m.marginLeft) * s, (l.yMin - m.marginTop) * s, (l.xMax + m.marginRight) * s, (l.yMax + m.marginBottom) * s);
            }
            float LeadMain(FlexKid k)
            {
                return horizontal ? k.X - Outer(k).xMin : k.Y - Outer(k).yMin;
            }
            float TrailMain(FlexKid k)
            {
                return horizontal ? Outer(k).xMax - (k.X + k.W) : Outer(k).yMax - (k.Y + k.H);
            }
            float LeadCross(FlexKid k)
            {
                return horizontal ? k.Y - Outer(k).yMin : k.X - Outer(k).xMin;
            }
            float TrailCross(FlexKid k)
            {
                return horizontal ? Outer(k).yMax - (k.Y + k.H) : Outer(k).xMax - (k.X + k.W);
            }
            if (margins)
            {
                // Each child with margins (or a text's padding) becomes its margin box.
                bool any = false;
                foreach (var k in order)
                {
                    var o = Outer(k);
                    float[] m = { k.Y - o.yMin, o.xMax - (k.X + k.W), o.yMax - (k.Y + k.H), k.X - o.xMin };
                    if (m.All(x => Mathf.Abs(x) < 0.05f))
                    {
                        continue;
                    }
                    if (m.Any(x => x < -0.05f))
                    {
                        why = (string)k.Node["name"] + " has a negative margin";
                        return null;
                    }
                    k.Margins = m.Select(R).ToArray();
                    k.X = R(o.xMin);
                    k.Y = R(o.yMin);
                    k.W = R(o.width);
                    k.H = R(o.height);
                    any = true;
                }
                if (!any)
                {
                    return null;
                }
            }
            float top = (rs.borderTopWidth + rs.paddingTop) * s;
            float right = (rs.borderRightWidth + rs.paddingRight) * s;
            float bottom = (rs.borderBottomWidth + rs.paddingBottom) * s;
            float left = (rs.borderLeftWidth + rs.paddingLeft) * s;
            var first = order[0];
            var last = order[order.Count - 1];
            p.PadMainStart = (horizontal ? left : top) + LeadMain(first);
            p.PadMainEnd = (horizontal ? right : bottom) + TrailMain(last);
            p.PadCrossStart = (horizontal ? top : left) + LeadCross(first);
            p.PadCrossEnd = (horizontal ? bottom : right) + TrailCross(first);
            if (order.Count > 1)
            {
                p.Gap = TrailMain(order[0]) + LeadMain(order[1]);
            }
            p.RowGap = TrailCross(first) + LeadCross(first);

            // The reversed order is laid out from the left/top, so flex-start packs at the end.
            var justify = rs.justifyContent;
            if ((justify == Justify.FlexStart) || (justify == Justify.FlexEnd))
            {
                p.Justify = ((justify == Justify.FlexEnd) != reverse) ? "END" : "START";
            }
            else if (justify == Justify.Center)
            {
                p.Justify = "CENTER";
            }
            else if (justify == Justify.SpaceBetween)
            {
                p.Justify = "SPACE_BETWEEN";
            }
            else
            {
                // space-around / space-evenly have no Flexma equivalent: the same spacing, fixed at this size.
                int count = order.Count;
                float main = horizontal ? w : h;
                float used = order.Sum(k => horizontal ? k.W : k.H) + (p.Gap * (count - 1));
                float free = main - p.PadMainStart - p.PadMainEnd - used;
                if (free > 0.01f)
                {
                    float edge = free / (count + 1);
                    float between = edge;
                    if (justify == Justify.SpaceAround)
                    {
                        edge = free / (2 * count);
                        between = free / count;
                    }
                    p.PadMainStart += edge;
                    p.PadMainEnd += edge;
                    p.Gap += between;
                }
            }
            if (rs.alignItems == Align.Center)
            {
                p.Align = "CENTER";
            }
            else if (rs.alignItems == Align.FlexEnd)
            {
                p.Align = "END";
            }

            bool hugW = sizing && Hugs(ve, true) && !(p.Wrap && horizontal);
            bool hugH = sizing && Hugs(ve, false) && !(p.Wrap && !horizontal);
            p.HugMain = horizontal ? hugW : hugH;
            p.HugCross = horizontal ? hugH : hugW;
            foreach (var k in order)
            {
                k.FillMain = sizing && !p.Wrap && !p.HugMain && (k.Element.resolvedStyle.flexGrow > 0);
                k.FillCross = sizing && Stretched(k.Element, !horizontal);
            }

            p.PadMainStart = R(p.PadMainStart);
            p.PadMainEnd = R(p.PadMainEnd);
            p.PadCrossStart = R(p.PadCrossStart);
            p.PadCrossEnd = R(p.PadCrossEnd);
            p.Gap = R(p.Gap);
            p.RowGap = R(p.RowGap);
            if (!Predict(p, w, h, out why))
            {
                return null;
            }
            return p;
        }

        static float R(float v)
        {
            return (float)Math.Round(v, 1);
        }

        void ApplyLayout(JObject frame, FlexPlan p, List<JObject> own, List<KeyValuePair<VisualElement, JObject>> kids)
        {
            frame["layout"] = p.Horizontal ? "HORIZONTAL" : "VERTICAL";
            if (p.Horizontal)
            {
                frame["padding"] = new JArray(Num(p.PadCrossStart), Num(p.PadMainEnd), Num(p.PadCrossEnd), Num(p.PadMainStart));
            }
            else
            {
                frame["padding"] = new JArray(Num(p.PadMainStart), Num(p.PadCrossEnd), Num(p.PadMainEnd), Num(p.PadCrossStart));
            }
            frame["gap"] = Num(p.Gap);
            frame["justify"] = p.Justify;
            frame["alignItems"] = p.Align;
            frame["layoutWrap"] = p.Wrap;
            if (p.Wrap)
            {
                frame["rowGap"] = Num(p.RowGap);
            }
            if (p.HugMain)
            {
                frame[p.Horizontal ? "layoutSizingHorizontal" : "layoutSizingVertical"] = "HUG";
            }
            if (p.HugCross)
            {
                frame[p.Horizontal ? "layoutSizingVertical" : "layoutSizingHorizontal"] = "HUG";
            }
            foreach (var k in p.Order)
            {
                if (k.FillMain)
                {
                    k.Node[p.Horizontal ? "layoutSizingHorizontal" : "layoutSizingVertical"] = "FILL";
                }
                if (k.FillCross)
                {
                    k.Node[p.Horizontal ? "layoutSizingVertical" : "layoutSizingHorizontal"] = "FILL";
                }
            }
            // What the element draws itself and absolutely positioned children stay out of the flow.
            foreach (var n in own)
            {
                n["absolute"] = true;
            }
            var slots = new List<int>();
            for (int i = 0; i < kids.Count; i++)
            {
                if (kids[i].Value == null)
                {
                    continue;
                }
                if (InFlow(kids[i].Key))
                {
                    slots.Add(i);
                }
                else
                {
                    kids[i].Value["absolute"] = true;
                }
            }
            // row-reverse / column-reverse: the flow children in visual order.
            for (int i = 0; i < slots.Count; i++)
            {
                var k = p.Order[i];
                kids[slots[i]] = new KeyValuePair<VisualElement, JObject>(k.Element, (k.Margins != null) ? MarginBox(k, p) : k.Node);
            }
        }

        /// <summary>A transparent frame the size of the child's margin box, padded by its margins, holding the child.</summary>
        JObject MarginBox(FlexKid k, FlexPlan p)
        {
            nodeCount++;
            var n = k.Node;
            var box = new JObject
            {
                ["type"] = "frame", ["key"] = (string)n["key"] + "#margin", ["name"] = (string)n["name"] + " (margin)",
                ["x"] = Num(k.X), ["y"] = Num(k.Y), ["w"] = Num(Mathf.Max(k.W, 1)), ["h"] = Num(Mathf.Max(k.H, 1)),
                ["fill"] = "none", ["stroke"] = "none", ["radius"] = 0, ["clip"] = false,
                ["layout"] = p.Horizontal ? "HORIZONTAL" : "VERTICAL", ["padding"] = new JArray(k.Margins.Select(Num)),
                ["gap"] = 0, ["justify"] = "START", ["alignItems"] = "START", ["layoutWrap"] = false,
                ["layoutSizingHorizontal"] = "HUG", ["layoutSizingVertical"] = "HUG"
            };
            string main = p.Horizontal ? "layoutSizingHorizontal" : "layoutSizingVertical";
            string cross = p.Horizontal ? "layoutSizingVertical" : "layoutSizingHorizontal";
            if (k.FillMain)
            {
                box[main] = "FILL";
            }
            else if ((string)n[main] == "FILL")
            {
                n[main] = "FIXED";
            }
            if (k.FillCross)
            {
                box[cross] = "FILL";
            }
            else if ((string)n[cross] == "FILL")
            {
                n[cross] = "FIXED";
            }
            if (n["visible"] != null)
            {
                box["visible"] = n["visible"];
                n.Remove("visible");
            }
            n["x"] = Num(k.Margins[3]);
            n["y"] = Num(k.Margins[0]);
            box["meta"] = new JObject { ["unity"] = new JObject { ["path"] = n.SelectToken("meta.unity.path"), ["box"] = "margin" } };
            box["children"] = new JArray(n);
            return box;
        }

        /// <summary>Flexma's auto layout (measure_hug, arrange, wrap) on the exported sizes: does every child land on Unity's rect?</summary>
        static bool Predict(FlexPlan p, float w, float h, out string why)
        {
            why = null;
            string miss = null;
            bool hz = p.Horizontal;
            var flow = p.Order.Where(k => k.Visible).ToList();
            float Main(FlexKid k)
            {
                return hz ? k.W : k.H;
            }
            float Cross(FlexKid k)
            {
                return hz ? k.H : k.W;
            }
            bool Near(float a, float b)
            {
                return Mathf.Abs(a - b) <= LayoutTolerance;
            }
            bool Lands(FlexKid k, float mainPos, float crossPos, float mainSize, float crossSize)
            {
                float x = hz ? mainPos : crossPos;
                float y = hz ? crossPos : mainPos;
                float cw = hz ? mainSize : crossSize;
                float ch = hz ? crossSize : mainSize;
                if (Near(x, k.X) && Near(y, k.Y) && Near(cw, k.W) && Near(ch, k.H))
                {
                    return true;
                }
                miss = (string)k.Node["name"] + " would be at " + R(x) + "," + R(y) + " " + R(cw) + "×" + R(ch) + " instead of " + k.X + "," + k.Y + " " + k.W + "×" + k.H;
                return false;
            }
            float actualMain = hz ? w : h;
            float actualCross = hz ? h : w;
            float padMain = p.PadMainStart + p.PadMainEnd;
            float padCross = p.PadCrossStart + p.PadCrossEnd;

            if (p.Wrap)
            {
                if (p.HugMain)
                {
                    why = "wrapping needs a fixed width";
                    return false;
                }
                float inner = actualMain - padMain;
                var lines = new List<List<FlexKid>>();
                var lineMain = new List<float>();
                var lineCross = new List<float>();
                foreach (var k in flow)
                {
                    float cm = Main(k);
                    float cc = k.FillCross ? 0 : Cross(k);
                    int li = lines.Count - 1;
                    if ((li >= 0) && (lineMain[li] + p.Gap + cm <= inner + 0.01f))
                    {
                        lineMain[li] += p.Gap + cm;
                        lineCross[li] = Mathf.Max(lineCross[li], cc);
                        lines[li].Add(k);
                    }
                    else
                    {
                        lines.Add(new List<FlexKid> { k });
                        lineMain.Add(cm);
                        lineCross.Add(cc);
                    }
                }
                if (p.HugCross && !Near(lineCross.Sum() + (p.RowGap * Mathf.Max(0, lines.Count - 1)) + padCross, actualCross))
                {
                    why = "hugging the lines gives another size";
                    return false;
                }
                float crossCursor = 0;
                for (int li = 0; li < lines.Count; li++)
                {
                    float cursor = 0;
                    float gap = p.Gap;
                    Distribute(p.Justify, Mathf.Max(0, inner - lineMain[li]), lines[li].Count, ref cursor, ref gap);
                    foreach (var k in lines[li])
                    {
                        float cc = k.FillCross ? lineCross[li] : Cross(k);
                        if (!Lands(k, p.PadMainStart + cursor, p.PadCrossStart + crossCursor + AlignOffset(p.Align, lineCross[li] - cc), Main(k), cc))
                        {
                            why = miss;
                            return false;
                        }
                        cursor += Main(k) + gap;
                    }
                    crossCursor += lineCross[li] + p.RowGap;
                }
                return true;
            }

            float main = 0;
            float cross = 0;
            float crossAll = 0;
            for (int i = 0; i < flow.Count; i++)
            {
                main += Main(flow[i]) + ((i > 0) ? p.Gap : 0);
                if (!flow[i].FillCross)
                {
                    cross = Mathf.Max(cross, Cross(flow[i]));
                }
                crossAll = Mathf.Max(crossAll, Cross(flow[i]));
            }
            if (cross == 0)
            {
                cross = crossAll;
            }
            if ((p.HugMain && !Near(main + padMain, actualMain)) || (p.HugCross && !Near(cross + padCross, actualCross)))
            {
                why = "hugging the children gives another size";
                return false;
            }
            float innerMain = Mathf.Max(0, actualMain - padMain);
            float innerCross = Mathf.Max(0, actualCross - padCross);
            float fixedMain = 0;
            int fillCount = 0;
            foreach (var k in flow)
            {
                if (k.FillMain && !p.HugMain)
                {
                    fillCount++;
                }
                else
                {
                    fixedMain += Main(k);
                }
            }
            float spacing = (flow.Count > 1) ? p.Gap * (flow.Count - 1) : 0;
            float fillEach = (fillCount > 0) ? Mathf.Max(0, (innerMain - fixedMain - spacing) / fillCount) : 0;
            var sizes = flow.Select(k => new Vector2((k.FillMain && !p.HugMain) ? fillEach : Main(k), k.FillCross ? innerCross : Cross(k))).ToList();
            float used = sizes.Sum(z => z.x) + spacing;
            float at = 0;
            float step = p.Gap;
            Distribute(p.Justify, Mathf.Max(0, innerMain - used), flow.Count, ref at, ref step);
            for (int i = 0; i < flow.Count; i++)
            {
                if (!Lands(flow[i], p.PadMainStart + at, p.PadCrossStart + AlignOffset(p.Align, innerCross - sizes[i].y), sizes[i].x, sizes[i].y))
                {
                    why = miss;
                    return false;
                }
                at += sizes[i].x + step;
            }
            return true;
        }

        static void Distribute(string justify, float free, int count, ref float cursor, ref float gap)
        {
            if (justify == "CENTER")
            {
                cursor = free / 2;
            }
            else if (justify == "END")
            {
                cursor = free;
            }
            else if ((justify == "SPACE_BETWEEN") && (count > 1))
            {
                gap += free / (count - 1);
            }
        }

        static float AlignOffset(string align, float room)
        {
            if (align == "CENTER")
            {
                return room / 2;
            }
            if (align == "END")
            {
                return room;
            }
            return 0;
        }

        /// <summary>Sized by its content along this axis (width/height auto, not stretched or grown by its parent, and
        /// not pinned between two insets).</summary>
        bool Hugs(VisualElement ve, bool horizontalAxis)
        {
            var size = ComputedLength(ve, horizontalAxis ? "width" : "height");
            if (size.HasValue && !IsAuto(size.Value))
            {
                return false;
            }
            if ((ve.resolvedStyle.position == Position.Absolute) && (InsetAxis(ComputedLength(ve, horizontalAxis ? "left" : "top"), ComputedLength(ve, horizontalAxis ? "right" : "bottom")) == "STRETCH"))
            {
                return false;
            }
            return !Stretched(ve, horizontalAxis);
        }

        /// <summary>Does the parent stretch this element along an axis: 100%, align-items/self stretch, or flex-grow.</summary>
        bool Stretched(VisualElement ve, bool horizontalAxis)
        {
            var parent = ve.hierarchy.parent;
            if (parent == null)
            {
                return false;
            }
            var size = ComputedLength(ve, horizontalAxis ? "width" : "height");
            bool auto = !size.HasValue || IsAuto(size.Value);
            if (!auto && (size.Value.unit == LengthUnit.Percent) && (size.Value.value >= 99.9f))
            {
                return true;
            }
            var rs = ve.resolvedStyle;
            if (rs.position == Position.Absolute)
            {
                return false;
            }
            var direction = parent.resolvedStyle.flexDirection;
            bool parentRow = (direction == FlexDirection.Row) || (direction == FlexDirection.RowReverse);
            if (horizontalAxis == parentRow)
            {
                return rs.flexGrow > 0;
            }
            if (!auto)
            {
                return false;
            }
            if (rs.alignSelf == Align.Stretch)
            {
                return true;
            }
            return (rs.alignSelf == Align.Auto) && (parent.resolvedStyle.alignItems == Align.Stretch);
        }

        /// <summary>Absolute elements follow their insets (left+right: stretch, right only: right...); elements in the
        /// flow stretch where flexbox stretches them, for when their parent keeps fixed positions.</summary>
        JToken ElementConstraints(VisualElement ve)
        {
            string h;
            string v;
            if (ve.resolvedStyle.position == Position.Absolute)
            {
                h = InsetAxis(ComputedLength(ve, "left"), ComputedLength(ve, "right"));
                v = InsetAxis(ComputedLength(ve, "top"), ComputedLength(ve, "bottom"));
            }
            else
            {
                h = Stretched(ve, true) ? "STRETCH" : "MIN";
                v = Stretched(ve, false) ? "STRETCH" : "MIN";
            }
            if ((h == "MIN") && (v == "MIN"))
            {
                return null;
            }
            return new JObject { ["horizontal"] = h, ["vertical"] = v };
        }

        string InsetAxis(Length? start, Length? end)
        {
            bool s = start.HasValue && !IsAuto(start.Value);
            bool e = end.HasValue && !IsAuto(end.Value);
            if ((s && (start.Value.unit == LengthUnit.Percent)) || (e && (end.Value.unit == LengthUnit.Percent)))
            {
                return "SCALE";
            }
            if (s && e)
            {
                return "STRETCH";
            }
            if (e)
            {
                return "MAX";
            }
            return "MIN";
        }

        // ── computed style (internal) ────────────────────────────

        /// <summary>A property of the element's computed style (keywords and units intact), or null if this Unity hides it.</summary>
        object ComputedValue(VisualElement ve, string property)
        {
            if (computedStyle == null)
            {
                return null;
            }
            object style = computedStyle.GetValue(ve);
            if (!computedProps.TryGetValue(property, out var info))
            {
                info = style.GetType().GetProperty(property, AnyMember);
                computedProps[property] = info;
            }
            if (info == null)
            {
                return null;
            }
            try
            {
                return info.GetValue(style);
            }
            catch (Exception)
            {
                return null;
            }
        }

        Length? ComputedLength(VisualElement ve, string property)
        {
            if (ComputedValue(ve, property) is Length length)
            {
                return length;
            }
            return null;
        }

        bool IsAuto(Length length)
        {
            if (lengthIsAuto == null)
            {
                return true;
            }
            return (bool)lengthIsAuto.Invoke(length, null);
        }

        static object Member(object o, string name)
        {
            if (o == null)
            {
                return null;
            }
            var type = o.GetType();
            var property = type.GetProperty(name, AnyMember);
            if (property != null)
            {
                return property.GetValue(o);
            }
            return type.GetField(name, AnyMember)?.GetValue(o);
        }

        // ── names, keys, meta ────────────────────────────────────

        static string FirstClass(VisualElement ve)
        {
            return ve.GetClasses().FirstOrDefault(c => !c.StartsWith("unity-"));
        }

        /// <summary>The element's name, else its first USS class, else its type ("Label").</summary>
        static string LayerName(VisualElement ve)
        {
            if (!string.IsNullOrEmpty(ve.name))
            {
                return ve.name;
            }
            string c = FirstClass(ve);
            if (c != null)
            {
                return "." + c;
            }
            return ve.GetType().Name;
        }

        static string Segment(VisualElement ve)
        {
            if (!string.IsNullOrEmpty(ve.name))
            {
                return ve.GetType().Name + "#" + ve.name;
            }
            string c = FirstClass(ve);
            if (c != null)
            {
                return ve.GetType().Name + "." + c;
            }
            return ve.GetType().Name;
        }

        /// <summary>Path from the document root: Type#name or Type.class per level, with [n] for repeats among siblings.</summary>
        string ElementPath(VisualElement ve)
        {
            var parts = new List<string>();
            for (var x = ve; (x != null) && (x != docRoot); x = x.hierarchy.parent)
            {
                string s = Segment(x);
                var parent = x.hierarchy.parent;
                int n = 0;
                if (parent != null)
                {
                    for (int i = 0; i < parent.hierarchy.childCount; i++)
                    {
                        var sibling = parent.hierarchy[i];
                        if (sibling == x)
                        {
                            break;
                        }
                        if (Segment(sibling) == s)
                        {
                            n++;
                        }
                    }
                }
                if (n > 0)
                {
                    s += "[" + n + "]";
                }
                parts.Insert(0, s);
            }
            return string.Join("/", parts);
        }

        /// <summary>A stable key: the document's GlobalObjectId plus the element's path.</summary>
        string ElementKey(VisualElement ve)
        {
            return "u" + Md5(Encoding.UTF8.GetBytes(docId + "|" + ElementPath(ve))).Substring(0, 16);
        }

        JObject ElementMeta(VisualElement ve)
        {
            var unity = new JObject { ["path"] = docPath + " :: " + ElementPath(ve), ["element"] = ve.GetType().Name };
            if (!string.IsNullOrEmpty(ve.name))
            {
                unity["name"] = ve.name;
            }
            var classes = ve.GetClasses().ToList();
            if (classes.Count > 0)
            {
                unity["classes"] = new JArray(classes);
            }
            if ((ve is TemplateContainer) && (templateSourceProperty != null) && (templateSourceProperty.GetValue(ve) is VisualTreeAsset template))
            {
                unity["template"] = AssetDatabase.GetAssetPath(template);
            }
            return new JObject { ["unity"] = unity };
        }
    }
}
#endif
