using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text.RegularExpressions;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using TMPro;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Experimental.Rendering;
using UnityEngine.SceneManagement;
using UnityEngine.UI;

/// <summary>
/// Exports the uGUI canvases of one or more scenes as editable Flexma layers (pages.json + images + fonts).
/// Open it from Tools > Flexma > UI Exporter (FlexmaUIExporterWindow); FlexmaUploader pushes the result to Flexma.
///
/// One page per scene, one frame per screen state:
///   "Canvas / Default"  what is visible when the scene starts
///   "Canvas / Panel"    a panel that starts hidden but is switched on at runtime: an inactive object referenced by
///                       a script or UnityEvent, or a big panel whose script hides it in Awake/Start
/// Inactive objects nothing references are treated as unused and skipped (listed in excluded.txt).
///
/// Image/RawImage -> image fills (9-slice, tint, preserve aspect), TMP/Text -> text, Mask/RectMask2D -> clip,
/// anchors -> constraints, CanvasGroup -> opacity, Shadow/Outline -> drop shadows, Selectable ColorTint -> tint.
/// Layout groups are rebuilt per screen on a copy of the canvas in a temporary scene, so open scenes are never modified.
/// Every layer carries a stable `key` (from the GameObject's GlobalObjectId), so a re-upload updates the same Flexma
/// layers in place and keeps their ids, comments and anything people added in Flexma.
///
/// Project hooks: subscribe to PoseScreen (show a screen the way the game does, e.g. a phone moved full screen) and
/// AddEffects (custom mesh effects as Flexma shadows/blurs) from an editor script in the project; see FlexmaProjectHooks.
/// </summary>
public static class FlexmaUIExporter
{
    public sealed class Options
    {
        public List<string> ScenePaths = new List<string>();
        public string OutputFolder;
        public Vector2 ScreenSize = new Vector2(1920, 1080);
        public int MaxImageSize = 2048;
        public string FrameBackground = "#2a2d33";
        /// <summary>Root canvases to leave out by name (editor overlays, debug consoles).</summary>
        public List<string> SkipCanvasNames = new List<string>();
        /// <summary>Export a Mask on a sprite as clip:"alpha" (clip by the sprite's shape) instead of a rect clip.</summary>
        public bool AlphaMasks = true;
        /// <summary>Export TMP/Text word wrapping and overflow (wrap:false, overflow CLIP/ELLIPSIS/TRUNCATE).</summary>
        public bool TextOverflow = true;
        /// <summary>Export GridLayoutGroups as Flexma grid layouts (editable grids) instead of fixed positions.</summary>
        public bool GridLayouts = true;
        /// <summary>Prefabs used two or more times become components on a "Components" page, and their uses become
        /// instances whose differences are overrides addressed by the component layer's key.</summary>
        public bool Components = true;
    }

    public sealed class Result
    {
        public string Folder;
        public int Pages, Frames, Nodes, Images, Fonts, Excluded;
        public readonly List<string> Warnings = new List<string>();

        public override string ToString()
        {
            string s = $"{Pages} pages, {Frames} frames, {Nodes} layers, {Images} images, {Fonts} fonts, {Excluded} unused objects skipped";
            if (Warnings.Count > 0)
            {
                s += $"\n{Warnings.Count} warnings:\n- " + string.Join("\n- ", Warnings.Take(20));
            }
            return s;
        }
    }

    public const string PagesFile = "pages.json";
    public const string ImagesFile = "images.json";
    public const string FontsFile = "fonts.json";
    public const string ImagesFolder = "images";

    public static string DefaultOutputFolder
    {
        get { return Path.GetFullPath(Path.Combine(Application.dataPath, "../Library/FlexmaExport")); }
    }

    /// <param name="progress">Called with a status and 0..1; return true to cancel.</param>
    public static Result Export(Options options, Func<string, float, bool> progress = null)
    {
        return new Exporter(options, progress).Run();
    }

    // ─────────────────────────────────────────────────────────────
    //  Project hooks
    // ─────────────────────────────────────────────────────────────

    /// <summary>
    /// Called for every frame after its objects are switched on and before layout runs. Use it when the game shows a
    /// screen differently from how it sits in the scene: move a phone to its full-screen pose, hide a bezel, etc.
    /// Every change goes through the context, which applies it to the export copy and undoes it before the next frame;
    /// the scene itself is never touched.
    /// </summary>
    public static event Action<ScreenContext> PoseScreen;

    /// <summary>Called for every exported layer, to add Flexma effects for custom mesh effects or shaders.</summary>
    public static event Action<LayerContext> AddEffects;

    /// <summary>Root canvases the project always leaves out, by name (editor overlays, debug consoles); add to it from the hooks.</summary>
    public static readonly List<string> SkippedCanvasNames = new List<string>();

    public sealed class ScreenContext
    {
        /// <summary>The original canvas (read only).</summary>
        public Canvas Canvas;
        /// <summary>The original panel this frame shows, or null for the canvas's default state.</summary>
        public Transform Screen;
        /// <summary>Set to a short phrase (e.g. "shown full screen") to note the pose in the frame's description and meta.</summary>
        public string Note;

        internal Func<Transform, Transform> CopyImpl;
        internal Action<RectTransform, RectTransform> MoveImpl;
        internal Action<Graphic> HideImpl;

        /// <summary>Does this frame show <paramref name="original"/> or something inside it? (e.g. "is this one of the phone's apps?")</summary>
        public bool ShowsInside(Transform original)
        {
            return (Screen != null) && (original != null) && Screen.IsChildOf(original);
        }

        /// <summary>Puts the copy of <paramref name="original"/> at the world position, rotation and scale of the copy of <paramref name="pose"/>.</summary>
        public void MoveTo(RectTransform original, RectTransform pose)
        {
            MoveImpl(original, pose);
        }

        /// <summary>Leaves this graphic (and a Mask that uses it) out of the frame.</summary>
        public void HideGraphic(Graphic original)
        {
            HideImpl(original);
        }

        /// <summary>The export copy of an original object, for anything MoveTo/HideGraphic don't cover. Restore what you change.</summary>
        public Transform Copy(Transform original)
        {
            return CopyImpl(original);
        }
    }

    public sealed class LayerContext
    {
        /// <summary>The original object (read only).</summary>
        public Transform Layer;
        /// <summary>Its RectTransform rect, in its own local units.</summary>
        public Rect Rect;
        /// <summary>Local units to Flexma pixels.</summary>
        public float Scale;
        /// <summary>Add Flexma effects here: {type:"DROP_SHADOW", x, y, blur, spread, color} (stacked, first lowest) or {type:"LAYER_BLUR", radius}.</summary>
        public JArray Effects;
    }

    // ─────────────────────────────────────────────────────────────
    //  Exporter
    // ─────────────────────────────────────────────────────────────

    private sealed class ImageEntry { public string Key, File, Name, Source, Hash; public int Width, Height; public float Scale; }
    private sealed class FontEntry { public string Key, Asset, Family, Style, Source; }
    private sealed class FrameEntry { public string Canvas, Name, Description; public float Width, Height; public JArray Nodes; public JObject Meta; }

    private sealed class Exporter
    {
        static readonly Regex StartMethod = new Regex(@"void\s+(Awake|Start)\s*\(\s*\)\s*\{");
        static readonly Regex HideSelf = new Regex(@"(^|[^\w\.])(this\.)?gameObject\.SetActive\(\s*false\s*\)");
        static readonly Regex RichTag = new Regex("<[^>]*>");
        static readonly Regex LineBreakTag = new Regex(@"<br\s*/?>", RegexOptions.IgnoreCase);

        readonly Options opt;
        readonly Func<string, float, bool> progress;
        readonly Result result = new Result();

        readonly Dictionary<GameObject, List<GameObject>> referrers = new Dictionary<GameObject, List<GameObject>>();
        readonly Dictionary<Type, bool> selfHidingTypes = new Dictionary<Type, bool>();
        readonly Dictionary<Transform, Matrix4x4> geoM = new Dictionary<Transform, Matrix4x4>();
        readonly Dictionary<Transform, Rect> geoR = new Dictionary<Transform, Rect>();
        readonly Dictionary<Transform, Transform> copyOf = new Dictionary<Transform, Transform>();
        readonly Dictionary<Graphic, Color> tint = new Dictionary<Graphic, Color>();
        readonly HashSet<Transform> selfHidden = new HashSet<Transform>();
        readonly HashSet<Transform> big = new HashSet<Transform>();
        readonly Dictionary<Transform, string> keys = new Dictionary<Transform, string>();

        // What the PoseScreen hook changed on the copy for the current frame, undone before the next one.
        sealed class MovedCopy { public RectTransform Copy; public Vector3 Position; public Quaternion Rotation; public Vector3 Scale; }
        readonly List<MovedCopy> movedCopies = new List<MovedCopy>();
        readonly HashSet<Graphic> disabledGraphics = new HashSet<Graphic>();
        readonly List<Graphic> disabledCopies = new List<Graphic>();
        Canvas currentCanvas;
        string screenNote;

        readonly Dictionary<string, ImageEntry> imagesByUid = new Dictionary<string, ImageEntry>();
        readonly HashSet<Texture> sharedTextures = new HashSet<Texture>();
        readonly Dictionary<string, ImageEntry> imagesByHash = new Dictionary<string, ImageEntry>();
        readonly List<ImageEntry> images = new List<ImageEntry>();
        readonly Dictionary<UnityEngine.Object, FontEntry> fonts = new Dictionary<UnityEngine.Object, FontEntry>();
        readonly List<string> excluded = new List<string>();
        readonly JArray pages = new JArray();

        Rect rootRect;
        Vector2 canvasSize;
        float refPPU;
        int nodeCount;
        string imageDir;

        public Exporter(Options options, Func<string, float, bool> progress)
        {
            opt = options;
            this.progress = progress;
        }

        public Result Run()
        {
            string folder = opt.OutputFolder;
            if (string.IsNullOrEmpty(folder))
            {
                folder = DefaultOutputFolder;
            }
            result.Folder = folder;
            imageDir = Path.Combine(folder, ImagesFolder);
            Directory.CreateDirectory(imageDir);
            foreach (var f in Directory.GetFiles(imageDir))
            {
                File.Delete(f);
            }

            var opened = new List<Scene>();
            var previousActive = SceneManager.GetActiveScene();
            Scene temp = default;
            try
            {
                foreach (var path in opt.ScenePaths)
                {
                    var s = SceneManager.GetSceneByPath(path);
                    if (s.IsValid() && s.isLoaded)
                    {
                        continue;
                    }
                    Report("Opening " + Path.GetFileNameWithoutExtension(path), 0f);
                    opened.Add(EditorSceneManager.OpenScene(path, OpenSceneMode.Additive));
                }

                Report("Scanning script references", 0.05f);
                BuildReferenceMap();
                FindSharedTextures();

                temp = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Additive);
                SceneManager.SetActiveScene(temp);

                for (int si = 0; si < opt.ScenePaths.Count; si++)
                {
                    var scene = SceneManager.GetSceneByPath(opt.ScenePaths[si]);
                    if (!scene.IsValid())
                    {
                        result.Warnings.Add("Scene not found: " + opt.ScenePaths[si]);
                        continue;
                    }
                    var page = new List<FrameEntry>();
                    var canvases = RootCanvases(scene);
                    for (int ci = 0; ci < canvases.Count; ci++)
                    {
                        float t = 0.1f + 0.85f * (si + ci / (float)Mathf.Max(1, canvases.Count)) / opt.ScenePaths.Count;
                        if (Report(scene.name + ": " + canvases[ci].name, t))
                        {
                            throw new OperationCanceledException("Export cancelled");
                        }
                        ExportCanvas(scene, canvases[ci], page, temp);
                    }
                    if (page.Count > 0)
                    {
                        pages.Add(new JObject { ["page"] = scene.name, ["nodes"] = LayoutPage(scene.name, page) });
                    }
                    result.Frames += page.Count;
                }
            }
            finally
            {
                if (previousActive.IsValid() && previousActive.isLoaded)
                {
                    SceneManager.SetActiveScene(previousActive);
                }
                if (temp.IsValid())
                {
                    EditorSceneManager.CloseScene(temp, true);
                }
                foreach (var s in opened)
                {
                    if (s.IsValid())
                    {
                        EditorSceneManager.CloseScene(s, true);
                    }
                }
            }

            WriteOutput(folder);
            return result;
        }

        bool Report(string status, float t)
        {
            if (progress == null)
            {
                return false;
            }
            return progress(status, t);
        }

        // ── references ───────────────────────────────────────────

        /// <summary>Who points at what: every object reference in every MonoBehaviour of the loaded scenes.</summary>
        void BuildReferenceMap()
        {
            for (int i = 0; i < SceneManager.sceneCount; i++)
            {
                var scene = SceneManager.GetSceneAt(i);
                if (!scene.isLoaded)
                {
                    continue;
                }
                foreach (var root in scene.GetRootGameObjects())
                {
                    foreach (var mb in root.GetComponentsInChildren<MonoBehaviour>(true))
                    {
                        if (mb == null)
                        {
                            continue;
                        }
                        var it = new SerializedObject(mb).GetIterator();
                        while (it.Next(true))
                        {
                            if (it.propertyType != SerializedPropertyType.ObjectReference)
                            {
                                continue;
                            }
                            var target = ReferencedGameObject(it.objectReferenceValue);
                            if ((target == null) || (target == mb.gameObject) || !target.scene.IsValid())
                            {
                                continue;
                            }
                            if (!referrers.TryGetValue(target, out var list))
                            {
                                list = new List<GameObject>();
                                referrers[target] = list;
                            }
                            if (!list.Contains(mb.gameObject))
                            {
                                list.Add(mb.gameObject);
                            }
                        }
                    }
                }
            }
        }

        static GameObject ReferencedGameObject(UnityEngine.Object o)
        {
            if (o is GameObject go)
            {
                return go;
            }
            if (o is Component c)
            {
                return c.gameObject;
            }
            return null;
        }

        /// <summary>Referenced by something outside its own subtree, so a script can switch it on.</summary>
        bool IsReferenced(Transform t)
        {
            if (!referrers.TryGetValue(t.gameObject, out var list))
            {
                return false;
            }
            return list.Any(r => (r != null) && !r.transform.IsChildOf(t));
        }

        string ReferencedBy(Transform t)
        {
            if (!referrers.TryGetValue(t.gameObject, out var list))
            {
                return "";
            }
            return string.Join(", ", list.Where(r => (r != null) && !r.transform.IsChildOf(t)).Select(r => r.name).Distinct().Take(4));
        }

        /// <summary>Does this script call gameObject.SetActive(false) on itself in Awake or Start?</summary>
        bool TypeHidesItself(MonoBehaviour mb)
        {
            var type = mb.GetType();
            if (selfHidingTypes.TryGetValue(type, out bool hides))
            {
                return hides;
            }
            hides = false;
            var script = MonoScript.FromMonoBehaviour(mb);
            if ((script != null) && AssetDatabase.GetAssetPath(script).StartsWith("Assets/"))
            {
                string text = script.text;
                foreach (Match m in StartMethod.Matches(text))
                {
                    int i = m.Index + m.Length;
                    int start = i;
                    int depth = 1;
                    while ((i < text.Length) && (depth > 0))
                    {
                        if (text[i] == '{')
                        {
                            depth++;
                        }
                        else if (text[i] == '}')
                        {
                            depth--;
                        }
                        i++;
                    }
                    foreach (var line in text.Substring(start, i - start).Split('\n'))
                    {
                        var l = line.Trim();
                        if (!l.StartsWith("//") && HideSelf.IsMatch(l))
                        {
                            hides = true;
                            break;
                        }
                    }
                    if (hides)
                    {
                        break;
                    }
                }
            }
            selfHidingTypes[type] = hides;
            return hides;
        }

        bool IsShown(Transform t)
        {
            return t.gameObject.activeSelf && !selfHidden.Contains(t);
        }

        bool IsUsed(Transform t)
        {
            return selfHidden.Contains(t) || IsReferenced(t);
        }

        // ── canvases ─────────────────────────────────────────────

        List<Canvas> RootCanvases(Scene scene)
        {
            var list = new List<Canvas>();
            foreach (var root in scene.GetRootGameObjects())
            {
                foreach (var c in root.GetComponentsInChildren<Canvas>(true))
                {
                    var parent = c.transform.parent;
                    if ((parent != null) && (parent.GetComponentInParent<Canvas>(true) != null))
                    {
                        continue;
                    }
                    if (opt.SkipCanvasNames.Contains(c.name) || SkippedCanvasNames.Contains(c.name))
                    {
                        continue;
                    }
                    list.Add(c);
                }
            }
            return list;
        }

        void ExportCanvas(Scene scene, Canvas canvas, List<FrameEntry> page, Scene temp)
        {
            var crt = (RectTransform)canvas.transform;
            if (!canvas.gameObject.activeSelf && !IsReferenced(crt))
            {
                excluded.Add(scene.name + " :: " + PathOf(crt) + " (inactive canvas, nothing references it)");
                return;
            }

            canvasSize = crt.rect.size;
            if (canvas.renderMode == RenderMode.ScreenSpaceOverlay)
            {
                canvasSize = opt.ScreenSize;
            }
            if ((canvasSize.x < 1) || (canvasSize.y < 1))
            {
                canvasSize = crt.sizeDelta;
            }
            if ((canvasSize.x < 1) || (canvasSize.y < 1))
            {
                canvasSize = opt.ScreenSize;
            }
            rootRect = new Rect(-crt.pivot.x * canvasSize.x, -crt.pivot.y * canvasSize.y, canvasSize.x, canvasSize.y);
            refPPU = 100f;
            var scaler = canvas.GetComponent<CanvasScaler>();
            if (scaler != null)
            {
                refPPU = scaler.referencePixelsPerUnit;
            }

            // A copy in the temp scene, so layout can be rebuilt for each screen state without touching the real scene.
            var copy = UnityEngine.Object.Instantiate(canvas.gameObject);
            try
            {
                if (copy.scene != temp)
                {
                    SceneManager.MoveGameObjectToScene(copy, temp);
                }
                copy.GetComponent<Canvas>().renderMode = RenderMode.WorldSpace;
                var copyRoot = (RectTransform)copy.transform;
                copyRoot.localScale = Vector3.one;
                copyRoot.localRotation = Quaternion.identity;
                copyRoot.position = Vector3.zero;
                copyRoot.sizeDelta = canvasSize;
                var originals = crt.GetComponentsInChildren<Transform>(true);
                var copies = copyRoot.GetComponentsInChildren<Transform>(true);
                if (originals.Length != copies.Length)
                {
                    result.Warnings.Add("Could not copy " + canvas.name);
                    return;
                }
                copyOf.Clear();
                for (int i = 0; i < originals.Length; i++)
                {
                    copyOf[originals[i]] = copies[i];
                }

                // Buttons, toggles and sliders tint their target graphic at runtime.
                tint.Clear();
                foreach (var sel in canvas.GetComponentsInChildren<Selectable>(true))
                {
                    if ((sel.transition != Selectable.Transition.ColorTint) || (sel.targetGraphic == null))
                    {
                        continue;
                    }
                    var cb = sel.colors;
                    Color c = cb.disabledColor;
                    if (sel.interactable)
                    {
                        c = cb.normalColor;
                    }
                    c *= cb.colorMultiplier;
                    if (!IsWhite(c))
                    {
                        tint[sel.targetGraphic] = c;
                    }
                }

                currentCanvas = canvas;

                // Default state first, to find big panels that hide themselves on start.
                selfHidden.Clear();
                big.Clear();
                SetState(originals, copies, null, crt);
                Layout(crt);
                foreach (var t in originals)
                {
                    if ((t == crt) || !t.gameObject.activeSelf || !(t is RectTransform))
                    {
                        continue;
                    }
                    bool hides = t.GetComponents<MonoBehaviour>().Any(mb => (mb != null) && mb.enabled && TypeHidesItself(mb));
                    if (hides && IsBig(t))
                    {
                        selfHidden.Add(t);
                    }
                }
                if (selfHidden.Count > 0)
                {
                    SetState(originals, copies, null, crt);
                    Layout(crt);
                }

                var roots = new List<Transform> { null };
                var names = new List<string> { canvas.name + " / Default" };
                Enumerate(crt, 0, canvas.name, roots, names, scene.name);

                for (int fi = 0; fi < roots.Count; fi++)
                {
                    var L = roots[fi];
                    SetState(originals, copies, L, crt);
                    Layout(crt);
                    nodeCount = 0;
                    var nodes = new JArray();
                    if (L == null)
                    {
                        foreach (Transform ch in crt)
                        {
                            if ((ch is RectTransform) && IsShown(ch))
                            {
                                Add(nodes, Render(ch, Vector2.zero, 0, false, null, 0, false));
                            }
                        }
                    }
                    else
                    {
                        var path = new List<Transform>();
                        for (var x = L; x != crt; x = x.parent)
                        {
                            path.Insert(0, x);
                        }
                        Add(nodes, Render(path[0], Vector2.zero, 0, false, path, 0, true));
                    }
                    if (nodeCount == 0)
                    {
                        continue;
                    }

                    page.Add(new FrameEntry
                    {
                        Canvas = canvas.name, Name = names[fi], Description = FrameDescription(scene, canvas, crt, L),
                        Width = canvasSize.x, Height = canvasSize.y, Nodes = nodes, Meta = FrameMeta(scene, crt, L)
                    });
                    result.Nodes += nodeCount;
                }
            }
            finally
            {
                ResetPose();
                UnityEngine.Object.DestroyImmediate(copy);
            }
        }

        string FrameDescription(Scene scene, Canvas canvas, Transform canvasRoot, Transform panel)
        {
            if (panel == null)
            {
                string d = "Unity " + scene.name + " :: " + PathOf(canvasRoot);
                if (!string.IsNullOrEmpty(screenNote))
                {
                    d += " (" + screenNote + ")";
                }
                if (!canvas.gameObject.activeSelf)
                {
                    return d + " (canvas inactive by default; switched on by: " + ReferencedBy(canvasRoot) + ")";
                }
                return d + " (default state, what is visible when the scene starts)";
            }
            string desc = "Unity " + scene.name + " :: " + PathOf(panel);
            if (!string.IsNullOrEmpty(screenNote))
            {
                desc += " (" + screenNote + ")";
            }
            if (selfHidden.Contains(panel))
            {
                return desc + " (active in the scene, but its script hides it on start; shown by script at runtime)";
            }
            return desc + " (inactive by default; switched on by: " + ReferencedBy(panel) + ")";
        }

        JObject FrameMeta(Scene scene, Transform canvasRoot, Transform panel)
        {
            var target = canvasRoot;
            string state = "default";
            if (panel != null)
            {
                target = panel;
                state = "panel";
                if (selfHidden.Contains(panel))
                {
                    state = "hidden-by-script-on-start";
                }
            }
            var unity = new JObject { ["scene"] = scene.path, ["path"] = PathOf(target), ["state"] = state };
            if (!string.IsNullOrEmpty(screenNote))
            {
                unity["pose"] = screenNote;
            }
            string by = ReferencedBy(target);
            if (by.Length > 0)
            {
                unity["switchedOnBy"] = by;
            }
            return new JObject { ["unity"] = unity };
        }

        void Enumerate(Transform t, int depth, string name, List<Transform> roots, List<string> names, string sceneName)
        {
            foreach (Transform ch in t)
            {
                if (!(ch is RectTransform))
                {
                    continue;
                }
                if (IsShown(ch))
                {
                    Enumerate(ch, depth, name, roots, names, sceneName);
                    continue;
                }
                if (!IsUsed(ch))
                {
                    excluded.Add(sceneName + " :: " + PathOf(ch));
                    continue;
                }
                if ((depth == 0) || IsBig(ch))
                {
                    big.Add(ch);
                    string n = name + " / " + ch.name;
                    string unique = n;
                    for (int dup = 2; names.Contains(unique); dup++)
                    {
                        unique = n + " (" + dup + ")";
                    }
                    roots.Add(ch);
                    names.Add(unique);
                    Enumerate(ch, depth + 1, unique, roots, names, sceneName);
                }
                else
                {
                    Enumerate(ch, depth, name, roots, names, sceneName);
                }
            }
        }

        /// <summary>Panel-sized: 15+ objects or at least 15% of the screen.</summary>
        bool IsBig(Transform t)
        {
            if (t.GetComponentsInChildren<RectTransform>(true).Length >= 15)
            {
                return true;
            }
            if (!geoR.ContainsKey(t))
            {
                return false;
            }
            Geo(t, geoR[t], out _, out var u, out var v, out _);
            return Mathf.Abs((u.x * v.y) - (u.y * v.x)) >= (0.15f * canvasSize.x * canvasSize.y);
        }

        /// <summary>Lets the project's PoseScreen hook pose the copy for this frame.</summary>
        void ApplyPose(Transform shown)
        {
            screenNote = null;
            if (PoseScreen == null)
            {
                return;
            }
            var context = new ScreenContext
            {
                Canvas = currentCanvas,
                Screen = shown,
                CopyImpl = CopyFor,
                MoveImpl = MoveCopy,
                HideImpl = HideCopy
            };
            try
            {
                PoseScreen(context);
            }
            catch (Exception e)
            {
                string where = currentCanvas.name;
                if (shown != null)
                {
                    where = PathOf(shown);
                }
                result.Warnings.Add("PoseScreen hook failed for " + where + ": " + e.Message);
            }
            screenNote = context.Note;
        }

        Transform CopyFor(Transform original)
        {
            if ((original == null) || !copyOf.TryGetValue(original, out var copy))
            {
                return null;
            }
            return copy;
        }

        void MoveCopy(RectTransform original, RectTransform pose)
        {
            var copy = CopyFor(original) as RectTransform;
            var target = CopyFor(pose);
            if ((copy == null) || (target == null))
            {
                return;
            }
            movedCopies.Add(new MovedCopy { Copy = copy, Position = copy.position, Rotation = copy.rotation, Scale = copy.localScale });
            copy.position = target.position;
            copy.rotation = target.rotation;
            copy.localScale = target.localScale;
        }

        void HideCopy(Graphic original)
        {
            if (original == null)
            {
                return;
            }
            disabledGraphics.Add(original);
            var copy = CopyFor(original.transform);
            if (copy == null)
            {
                return;
            }
            var copied = copy.GetComponents<Graphic>().FirstOrDefault(g => g.GetType() == original.GetType());
            if ((copied != null) && copied.enabled)
            {
                copied.enabled = false;
                disabledCopies.Add(copied);
            }
        }

        void ResetPose()
        {
            for (int i = movedCopies.Count - 1; i >= 0; i--)
            {
                var m = movedCopies[i];
                if (m.Copy != null)
                {
                    m.Copy.position = m.Position;
                    m.Copy.rotation = m.Rotation;
                    m.Copy.localScale = m.Scale;
                }
            }
            movedCopies.Clear();
            foreach (var g in disabledCopies)
            {
                if (g != null)
                {
                    g.enabled = true;
                }
            }
            disabledCopies.Clear();
            disabledGraphics.Clear();
            screenNote = null;
        }

        /// <summary>Activate the copy like the game would for this screen, then let uGUI lay it out.</summary>
        void SetState(Transform[] originals, Transform[] copies, Transform shown, Transform canvasRoot)
        {
            ResetPose();
            for (int i = 1; i < originals.Length; i++)
            {
                bool want = IsShown(originals[i]);
                if (copies[i].gameObject.activeSelf != want)
                {
                    copies[i].gameObject.SetActive(want);
                }
            }
            copies[0].gameObject.SetActive(true);
            for (var x = shown; (x != null) && (x != canvasRoot); x = x.parent)
            {
                if (!copyOf[x].gameObject.activeSelf)
                {
                    copyOf[x].gameObject.SetActive(true);
                }
            }
            ApplyPose(shown);

            var root = (RectTransform)copies[0];
            Canvas.ForceUpdateCanvases();
            for (int pass = 0; pass < 2; pass++)
            {
                foreach (var tmp in root.GetComponentsInChildren<TMP_Text>(false))
                {
                    if (tmp.enableAutoSizing)
                    {
                        tmp.ForceMeshUpdate();
                    }
                }
                LayoutRebuilder.ForceRebuildLayoutImmediate(root);
                Canvas.ForceUpdateCanvases();
            }
        }

        // ── geometry ─────────────────────────────────────────────

        /// <summary>Canvas-space matrix and local rect for every RectTransform, read from the laid-out copy.</summary>
        void Layout(RectTransform canvasRoot)
        {
            geoM.Clear();
            geoR.Clear();
            geoM[canvasRoot] = Matrix4x4.identity;
            geoR[canvasRoot] = rootRect;
            LayoutChildren(canvasRoot, Matrix4x4.identity, rootRect);
        }

        void LayoutChildren(RectTransform t, Matrix4x4 parentMatrix, Rect parentRect)
        {
            foreach (Transform child in t)
            {
                if (!(child is RectTransform original))
                {
                    continue;
                }
                var c = original;
                if (copyOf.TryGetValue(original, out var copied))
                {
                    c = (RectTransform)copied;
                }
                Vector2 aMin = parentRect.min + Vector2.Scale(c.anchorMin, parentRect.size);
                Vector2 aMax = parentRect.min + Vector2.Scale(c.anchorMax, parentRect.size);
                Vector2 rMin = aMin + c.offsetMin;
                Vector2 size = (aMax + c.offsetMax) - rMin;
                Vector2 pivot = rMin + Vector2.Scale(c.pivot, size);
                var m = parentMatrix * Matrix4x4.TRS(new Vector3(pivot.x, pivot.y, 0f), c.localRotation, c.localScale);
                var r = new Rect(-c.pivot.x * size.x, -c.pivot.y * size.y, size.x, size.y);
                geoM[original] = m;
                geoR[original] = r;
                LayoutChildren(original, m, r);
            }
        }

        Vector2 ToPx(Vector3 p)
        {
            return new Vector2(p.x - rootRect.xMin, rootRect.yMax - p.y);
        }

        /// <summary>Top-left corner, width/height edge vectors and rotation of a rect, in canvas pixels (y down).</summary>
        void Geo(Transform t, Rect r, out Vector2 topLeft, out Vector2 u, out Vector2 v, out float angle)
        {
            var m = geoM[t];
            topLeft = ToPx(m.MultiplyPoint3x4(new Vector3(r.xMin, r.yMax, 0)));
            u = ToPx(m.MultiplyPoint3x4(new Vector3(r.xMax, r.yMax, 0))) - topLeft;
            v = ToPx(m.MultiplyPoint3x4(new Vector3(r.xMin, r.yMin, 0))) - topLeft;
            var axis = ToPx(m.MultiplyPoint3x4(Vector3.right)) - ToPx(m.MultiplyPoint3x4(Vector3.zero));
            angle = Mathf.Atan2(axis.y, axis.x) * Mathf.Rad2Deg;
        }

        /// <summary>Canvas pixels per local unit along the rect's height (falls back to width).</summary>
        static float ScaleOf(Rect r, Vector2 u, Vector2 v)
        {
            if (r.height > 0.01f)
            {
                return v.magnitude / r.height;
            }
            if (r.width > 0.01f)
            {
                return u.magnitude / r.width;
            }
            return 1f;
        }

        /// <summary>x/y/w/h (and rotation, flip) relative to the parent layer's top-left and rotation.</summary>
        static void Place(JObject n, Vector2 topLeft, Vector2 u, Vector2 v, float angle, Vector2 parentTopLeft, float parentAngle)
        {
            float w = u.magnitude;
            float h = v.magnitude;
            var d = topLeft + ((u + v) * 0.5f) - parentTopLeft;
            float rad = parentAngle * Mathf.Deg2Rad;
            float cs = Mathf.Cos(rad);
            float sn = Mathf.Sin(rad);
            float dx = (d.x * cs) + (d.y * sn);
            float dy = (-d.x * sn) + (d.y * cs);
            n["x"] = Num(dx - (w / 2));
            n["y"] = Num(dy - (h / 2));
            n["w"] = Num(Mathf.Max(w, 1));
            n["h"] = Num(Mathf.Max(h, 1));
            float rot = Mathf.DeltaAngle(0, angle - parentAngle);
            if (Mathf.Abs(rot) > 0.05f)
            {
                n["rotation"] = Num(rot);
            }
            if (((u.x * v.y) - (u.y * v.x)) < -0.0001f)
            {
                n["flipY"] = true;
            }
        }

        // ── layers ───────────────────────────────────────────────

        JObject Render(Transform t, Vector2 parentTopLeft, float parentAngle, bool hidden, List<Transform> path, int pathIndex, bool panelFrame)
        {
            if (((t.gameObject.hideFlags & HideFlags.HideInHierarchy) != 0) || (t.GetComponent<TMP_SubMeshUI>() != null))
            {
                return null;
            }
            if (!(t is RectTransform rt) || !geoR.ContainsKey(t))
            {
                return null;
            }
            var r = geoR[t];
            Geo(t, r, out var tl, out var u, out var v, out float angle);
            float scale = ScaleOf(r, u, v);

            var g = t.GetComponents<Graphic>().FirstOrDefault(x => (x != null) && x.enabled && !disabledGraphics.Contains(x));
            var mask = t.GetComponent<Mask>();
            bool maskOn = (mask != null) && mask.enabled && !((mask.graphic != null) && disabledGraphics.Contains(mask.graphic));
            var rectMask = t.GetComponent<RectMask2D>();
            bool clip = maskOn || ((rectMask != null) && rectMask.enabled);
            bool showGraphic = (g != null) && !(maskOn && !mask.showMaskGraphic) && (Col(g).a > 0.004f);
            bool isText = showGraphic && ((g is TMP_Text) || (g is Text));

            // Children: only the panel path for ancestors of a panel frame, otherwise what is shown in this state.
            var kids = new JArray();
            if ((path != null) && (pathIndex < path.Count - 1))
            {
                Add(kids, Render(path[pathIndex + 1], tl, angle, false, path, pathIndex + 1, panelFrame));
            }
            else
            {
                foreach (Transform ch in t)
                {
                    if (!(ch is RectTransform))
                    {
                        continue;
                    }
                    if (IsShown(ch))
                    {
                        Add(kids, Render(ch, tl, angle, false, null, 0, panelFrame));
                    }
                    else if (panelFrame && IsUsed(ch) && !big.Contains(ch))
                    {
                        Add(kids, Render(ch, tl, angle, true, null, 0, panelFrame));
                    }
                }
            }

            var tail = new JObject();
            var constraints = Constraints(rt);
            if (constraints != null)
            {
                tail["constraints"] = constraints;
            }
            if (hidden)
            {
                tail["visible"] = false;
            }
            var group = t.GetComponent<CanvasGroup>();
            if ((group != null) && group.enabled && (group.alpha < 0.999f))
            {
                tail["opacity"] = Math.Round(group.alpha, 2);
            }

            if (isText)
            {
                JObject text;
                if (kids.Count == 0)
                {
                    text = Text(t, g, parentTopLeft, parentAngle, t.name);
                }
                else
                {
                    text = Text(t, g, tl, angle, t.name + " (text)");
                }
                if (text != null)
                {
                    if (kids.Count == 0)
                    {
                        text["key"] = KeyOf(t);
                        Merge(text, tail);
                        text["meta"] = Meta(t);
                        return text;
                    }
                    text["key"] = KeyOf(t) + "#text";
                    kids.Insert(0, text);
                    var wrapper = Frame(t.name, tl, u, v, angle, parentTopLeft, parentAngle, null, clip, false, false, null, tail, kids);
                    wrapper["key"] = KeyOf(t);
                    wrapper["meta"] = Meta(t);
                    return wrapper;
                }
                showGraphic = false;
            }

            string note = null;
            JToken paint = null;
            if (showGraphic)
            {
                paint = Paint(g, scale, out note);
            }
            if ((paint != null) && (kids.Count == 0))
            {
                nodeCount++;
                var n = new JObject { ["type"] = "rect", ["key"] = KeyOf(t), ["name"] = t.name };
                Place(n, tl, u, v, angle, parentTopLeft, parentAngle);
                n["fill"] = paint;
                var fx = Effects(t, scale);
                if (fx != null)
                {
                    n["effects"] = fx;
                }
                Merge(n, tail);
                if (note != null)
                {
                    n["description"] = note;
                }
                n["meta"] = Meta(t);
                return n;
            }
            if (kids.Count == 0)
            {
                return null;
            }

            // A Mask clips its children to the shape of its graphic: the sprite's alpha, even when the graphic itself is hidden.
            bool alphaMask = false;
            bool hideFill = false;
            if (opt.AlphaMasks && maskOn && ((g is Image) || (g is RawImage)))
            {
                var maskPaint = paint;
                if (maskPaint == null)
                {
                    maskPaint = Paint(g, scale, out _);
                }
                if (maskPaint is JObject)
                {
                    alphaMask = true;
                    hideFill = !mask.showMaskGraphic;
                    paint = maskPaint;
                }
            }
            var frame = Frame(t.name, tl, u, v, angle, parentTopLeft, parentAngle, paint, clip, alphaMask, hideFill, note, tail, kids);
            frame["key"] = KeyOf(t);
            frame["meta"] = Meta(t);
            ApplyGrid(frame, t, scale);
            if ((paint != null) && !hideFill)
            {
                var fx = Effects(t, scale);
                if (fx != null)
                {
                    frame["effects"] = fx;
                }
            }
            return frame;
        }

        JObject Frame(string name, Vector2 tl, Vector2 u, Vector2 v, float angle, Vector2 parentTopLeft, float parentAngle, JToken paint, bool clip, bool alphaMask, bool hideFill, string note, JObject tail, JArray kids)
        {
            nodeCount++;
            var n = new JObject { ["type"] = "frame", ["name"] = name };
            Place(n, tl, u, v, angle, parentTopLeft, parentAngle);
            if (paint != null)
            {
                n["fill"] = paint;
            }
            else
            {
                n["fill"] = "none";
            }
            if (alphaMask)
            {
                n["clip"] = "alpha";
                if (hideFill)
                {
                    n["hideFill"] = true;
                }
            }
            else
            {
                n["clip"] = clip;
            }
            Merge(n, tail);
            if (!string.IsNullOrEmpty(note))
            {
                n["description"] = note;
            }
            n["children"] = kids;
            return n;
        }

        /// <summary>The texture a sprite's rect refers to. In the editor a packed sprite reports its atlas instead.</summary>
        static Texture SourceTexture(Sprite sprite)
        {
            if (sprite.packed)
            {
                var original = AssetDatabase.LoadAllAssetsAtPath(AssetDatabase.GetAssetPath(sprite)).OfType<Texture2D>().FirstOrDefault();
                if (original != null)
                {
                    return original;
                }
            }
            return sprite.texture;
        }

        /// <summary>Textures that two or more exported sprites come from (and fit the size cap): uploaded once and cropped.</summary>
        void FindSharedTextures()
        {
            var rects = new Dictionary<Texture, HashSet<Rect>>();
            foreach (var path in opt.ScenePaths)
            {
                var scene = SceneManager.GetSceneByPath(path);
                if (!scene.IsValid())
                {
                    continue;
                }
                foreach (var canvas in RootCanvases(scene))
                {
                    foreach (var img in canvas.GetComponentsInChildren<Image>(true))
                    {
                        if (img.sprite == null)
                        {
                            continue;
                        }
                        var tex = SourceTexture(img.sprite);
                        if (!rects.TryGetValue(tex, out var set))
                        {
                            set = new HashSet<Rect>();
                            rects[tex] = set;
                        }
                        set.Add(img.sprite.rect);
                    }
                }
            }
            sharedTextures.Clear();
            foreach (var kv in rects)
            {
                if ((kv.Value.Count >= 2) && (Mathf.Max(kv.Key.width, kv.Key.height) <= opt.MaxImageSize))
                {
                    sharedTextures.Add(kv.Key);
                }
            }
        }

        /// <summary>A Unity GridLayoutGroup as a Flexma grid, so the cells stay editable as a grid.</summary>
        void ApplyGrid(JObject n, Transform t, float scale)
        {
            var grid = t.GetComponent<GridLayoutGroup>();
            if (!opt.GridLayouts || (grid == null) || !grid.enabled)
            {
                return;
            }
            n["layout"] = "grid";
            if (grid.constraint == GridLayoutGroup.Constraint.FixedColumnCount)
            {
                n["columns"] = grid.constraintCount;
            }
            else if (grid.constraint == GridLayoutGroup.Constraint.FixedRowCount)
            {
                n["rows"] = grid.constraintCount;
            }
            n["cellSize"] = new JArray(Num(grid.cellSize.x * scale), Num(grid.cellSize.y * scale));
            n["gap"] = Num(grid.spacing.x * scale);
            n["rowGap"] = Num(grid.spacing.y * scale);
            var pad = grid.padding;
            n["padding"] = new JArray(Num(pad.top * scale), Num(pad.right * scale), Num(pad.bottom * scale), Num(pad.left * scale));
            string corner = "UPPER_LEFT";
            if (grid.startCorner == GridLayoutGroup.Corner.UpperRight)
            {
                corner = "UPPER_RIGHT";
            }
            else if (grid.startCorner == GridLayoutGroup.Corner.LowerLeft)
            {
                corner = "LOWER_LEFT";
            }
            else if (grid.startCorner == GridLayoutGroup.Corner.LowerRight)
            {
                corner = "LOWER_RIGHT";
            }
            n["startCorner"] = corner;
            if (grid.startAxis == GridLayoutGroup.Axis.Vertical)
            {
                n["startAxis"] = "vertical";
            }
            else
            {
                n["startAxis"] = "horizontal";
            }
            int a = (int)grid.childAlignment;
            string[] steps = { "start", "center", "end" };
            n["justify"] = steps[a % 3];
            n["alignItems"] = steps[a / 3];

            // Layers the grid doesn't place in Unity (ignoreLayout, or inactive and shown hidden) keep their own position.
            foreach (JObject child in (JArray)n["children"])
            {
                if ((child["visible"] != null) && !(bool)child["visible"])
                {
                    child["absolute"] = true;
                }
            }
            foreach (Transform ch in t)
            {
                var element = ch.GetComponent<LayoutElement>();
                if ((element == null) || !element.ignoreLayout)
                {
                    continue;
                }
                string key = KeyOf(ch);
                foreach (JObject child in (JArray)n["children"])
                {
                    if ((string)child["key"] == key)
                    {
                        child["absolute"] = true;
                    }
                }
            }
        }

        JToken Paint(Graphic g, float scale, out string note)
        {
            note = null;
            var color = Col(g);
            if (g is Image img)
            {
                var sprite = img.sprite;
                if (sprite == null)
                {
                    return Hex(color);
                }
                var source = SourceTexture(sprite);
                ImageEntry image;
                JArray crop = null;
                if (sharedTextures.Contains(source))
                {
                    // A sheet several sprites come from: upload it once and crop each sprite out of it (y from the top).
                    image = ExportImage(source, new Rect(0, 0, source.width, source.height), "tex" + source.GetInstanceID(), source.name);
                    var r = sprite.rect;
                    float ck = image.Scale;
                    crop = new JArray(Num(r.x * ck), Num((source.height - r.y - r.height) * ck), Num(r.width * ck), Num(r.height * ck));
                }
                else
                {
                    image = ExportImage(source, sprite.rect, "sp" + sprite.GetInstanceID(), sprite.name);
                }
                var p = new JObject { ["type"] = "IMAGE", ["asset"] = "@" + image.Key };
                if (crop != null)
                {
                    p["crop"] = crop;
                }
                if (!IsWhite(color))
                {
                    p["tint"] = Hex(color);
                }
                var b = sprite.border;
                if ((img.type == Image.Type.Sliced) && (b.sqrMagnitude > 0))
                {
                    float k = image.Scale;
                    var slice = new JObject
                    {
                        ["top"] = Num(b.w * k), ["right"] = Num(b.z * k), ["bottom"] = Num(b.y * k), ["left"] = Num(b.x * k),
                        ["scale"] = Math.Round(scale * refPPU / (sprite.pixelsPerUnit * img.pixelsPerUnitMultiplier) / k, 3)
                    };
                    if (!img.fillCenter)
                    {
                        slice["fillCenter"] = false;
                    }
                    p["slice"] = slice;
                }
                else if (img.preserveAspect)
                {
                    p["fit"] = "FIT";
                }
                else
                {
                    p["fit"] = "STRETCH";
                }
                if (img.type == Image.Type.Tiled)
                {
                    p.Remove("slice");
                    p["fit"] = "TILE";
                    p["tileScale"] = Math.Round(scale * refPPU / (sprite.pixelsPerUnit * img.pixelsPerUnitMultiplier) / image.Scale, 3);
                }
                if (img.type == Image.Type.Filled)
                {
                    p["filled"] = new JObject
                    {
                        ["method"] = FillMethodName(img.fillMethod),
                        ["amount"] = Math.Round(img.fillAmount, 4),
                        ["origin"] = img.fillOrigin,
                        ["clockwise"] = img.fillClockwise
                    };
                }
                return p;
            }
            if (g is RawImage raw)
            {
                var tex = raw.texture;
                if (tex == null)
                {
                    return Hex(color);
                }
                if (!(tex is Texture2D))
                {
                    note = "RawImage shows a runtime texture: " + tex.name + " (" + tex.GetType().Name + ")";
                    return "#2b3240";
                }
                var uv = raw.uvRect;
                var region = new Rect(uv.x * tex.width, uv.y * tex.height, uv.width * tex.width, uv.height * tex.height);
                if ((uv.xMin < -0.001f) || (uv.yMin < -0.001f) || (uv.xMax > 1.001f) || (uv.yMax > 1.001f))
                {
                    region = new Rect(0, 0, tex.width, tex.height);
                }
                var image = ExportImage(tex, region, "tx" + tex.GetInstanceID() + "_" + region, tex.name);
                var p = new JObject { ["type"] = "IMAGE", ["asset"] = "@" + image.Key, ["fit"] = "STRETCH" };
                if (!IsWhite(color))
                {
                    p["tint"] = Hex(color);
                }
                return p;
            }
            note = "Unity Graphic: " + g.GetType().Name;
            return "#ffffff22";
        }

        JObject Text(Transform t, Graphic g, Vector2 parentTopLeft, float parentAngle, string name)
        {
            var r = geoR[t];
            string text;
            string fontKey;
            float size;
            float lineHeight;
            float letterSpacing = 0;
            float padRatio = 0;
            bool bold;
            bool italic;
            bool underline = false;
            bool strike = false;
            string align = "LEFT";
            string valign = "TOP";
            bool autoSize = false;
            float sizeMin = 0;
            bool wrap = true;
            string overflow = null;
            var color = Col(g);
            var margin = Vector4.zero;
            JToken gradient = null;
            Material material = null;

            if (g is TMP_Text tmp)
            {
                text = tmp.text;
                if (string.IsNullOrEmpty(text))
                {
                    return null;
                }
                if (tmp.richText)
                {
                    text = LineBreakTag.Replace(text, "\n");
                    text = RichTag.Replace(text, "");
                }
                // TMP doesn't start a new line for one trailing line break ("A\n" lays out as one centered line),
                // while Flexma draws it as an empty last line, so drop it. Legacy Text keeps it, like Flexma.
                text = text.Replace("\r\n", "\n");
                if (text.EndsWith("\n"))
                {
                    text = text.Substring(0, text.Length - 1);
                }
                if (text.Length == 0)
                {
                    return null;
                }
                var style = tmp.fontStyle;
                if (((style & FontStyles.UpperCase) != 0) || ((style & FontStyles.SmallCaps) != 0))
                {
                    text = text.ToUpperInvariant();
                }
                if ((style & FontStyles.LowerCase) != 0)
                {
                    text = text.ToLowerInvariant();
                }
                bold = (style & FontStyles.Bold) != 0;
                italic = (style & FontStyles.Italic) != 0;
                underline = (style & FontStyles.Underline) != 0;
                strike = (style & FontStyles.Strikethrough) != 0;
                size = tmp.fontSize;
                if (tmp.enableAutoSizing)
                {
                    // Flexma shrinks from the max size to fit the box, like TMP.
                    autoSize = true;
                    sizeMin = tmp.fontSizeMin;
                    size = tmp.fontSizeMax;
                }
                fontKey = FontKey(tmp.font);
                var h = tmp.horizontalAlignment;
                if ((h == HorizontalAlignmentOptions.Center) || (h == HorizontalAlignmentOptions.Geometry))
                {
                    align = "CENTER";
                }
                else if (h == HorizontalAlignmentOptions.Right)
                {
                    align = "RIGHT";
                }
                var va = tmp.verticalAlignment;
                if (va == VerticalAlignmentOptions.Bottom)
                {
                    valign = "BOTTOM";
                }
                else if (va != VerticalAlignmentOptions.Top)
                {
                    valign = "CENTER";
                }
                if ((tmp.font != null) && (tmp.font.faceInfo.pointSize > 0))
                {
                    lineHeight = tmp.font.faceInfo.lineHeight / (float)tmp.font.faceInfo.pointSize * size;
                    padRatio = tmp.font.atlasPadding / (float)tmp.font.faceInfo.pointSize;
                }
                else
                {
                    lineHeight = size * 1.2f;
                }
                lineHeight += tmp.lineSpacing * 0.01f * size;
                letterSpacing = tmp.characterSpacing * 0.01f * size;
                margin = tmp.margin;
                wrap = WordWraps(tmp);
                if ((tmp.overflowMode == TextOverflowModes.Masking) || (tmp.overflowMode == TextOverflowModes.ScrollRect))
                {
                    overflow = "CLIP";
                }
                else if (tmp.overflowMode == TextOverflowModes.Ellipsis)
                {
                    overflow = "ELLIPSIS";
                }
                else if (tmp.overflowMode == TextOverflowModes.Truncate)
                {
                    overflow = "TRUNCATE";
                }
                if (tmp.enableVertexGradient)
                {
                    var cg = tmp.colorGradient;
                    gradient = new JObject
                    {
                        ["type"] = "LINEAR_GRADIENT", ["angle"] = 90,
                        ["stops"] = new JArray(
                            new JObject { ["position"] = 0, ["color"] = Hex(cg.topLeft * color) },
                            new JObject { ["position"] = 1, ["color"] = Hex(cg.bottomLeft * color) })
                    };
                }
                material = tmp.fontSharedMaterial;
            }
            else
            {
                var ut = (Text)g;
                text = ut.text;
                if (string.IsNullOrEmpty(text))
                {
                    return null;
                }
                if (ut.supportRichText)
                {
                    text = RichTag.Replace(text, "");
                }
                size = ut.fontSize;
                if (ut.resizeTextForBestFit)
                {
                    autoSize = true;
                    sizeMin = ut.resizeTextMinSize;
                    size = ut.resizeTextMaxSize;
                }
                fontKey = FontKey(ut.font);
                bold = (ut.fontStyle == FontStyle.Bold) || (ut.fontStyle == FontStyle.BoldAndItalic);
                italic = (ut.fontStyle == FontStyle.Italic) || (ut.fontStyle == FontStyle.BoldAndItalic);
                int a = (int)ut.alignment;
                if ((a % 3) == 1)
                {
                    align = "CENTER";
                }
                else if ((a % 3) == 2)
                {
                    align = "RIGHT";
                }
                if ((a / 3) == 1)
                {
                    valign = "CENTER";
                }
                else if ((a / 3) == 2)
                {
                    valign = "BOTTOM";
                }
                lineHeight = size * 1.15f * ut.lineSpacing;
                wrap = ut.horizontalOverflow == HorizontalWrapMode.Wrap;
                if (ut.verticalOverflow == VerticalWrapMode.Truncate)
                {
                    overflow = "TRUNCATE";
                }
            }

            var box = new Rect(r.xMin + margin.x, r.yMin + margin.w, Mathf.Max(0, r.width - margin.x - margin.z), Mathf.Max(0, r.height - margin.y - margin.w));
            Geo(t, box, out var tl, out var u, out var v, out float angle);
            Geo(t, r, out _, out var u0, out var v0, out _);
            float scale = ScaleOf(r, u0, v0);

            nodeCount++;
            var n = new JObject { ["type"] = "text", ["name"] = name };
            Place(n, tl, u, v, angle, parentTopLeft, parentAngle);
            n["text"] = text;
            n["size"] = Num(size * scale);
            if (autoSize)
            {
                n["autoSize"] = new JArray(Num(sizeMin * scale), Num(size * scale));
            }
            if (fontKey != null)
            {
                n["font"] = "@" + fontKey;
            }
            if (bold)
            {
                n["weight"] = 700;
            }
            if (italic)
            {
                n["italic"] = true;
            }
            if (underline)
            {
                n["textDecoration"] = "UNDERLINE";
            }
            else if (strike)
            {
                n["textDecoration"] = "STRIKETHROUGH";
            }
            if (gradient != null)
            {
                n["fill"] = gradient;
            }
            else
            {
                n["color"] = Hex(color);
            }
            if (align != "LEFT")
            {
                n["align"] = align;
            }
            if (valign != "TOP")
            {
                n["verticalAlign"] = valign;
            }
            n["lineHeight"] = Num(lineHeight * scale);
            if (Mathf.Abs(letterSpacing) > 0.05f)
            {
                n["letterSpacing"] = Num(letterSpacing * scale);
            }
            n["textAutoResize"] = "NONE";
            if (opt.TextOverflow)
            {
                if (!wrap)
                {
                    n["wrap"] = false;
                }
                if (overflow != null)
                {
                    n["overflow"] = overflow;
                }
            }

            // TMP outline and underlay live in the font material.
            JArray effects = null;
            if (material != null)
            {
                if (material.HasProperty("_OutlineWidth") && material.HasProperty("_OutlineColor") && (material.GetFloat("_OutlineWidth") > 0.02f))
                {
                    var oc = material.GetColor("_OutlineColor");
                    if (oc.a > 0.02f)
                    {
                        n["stroke"] = Hex(oc);
                        n["strokeWeight"] = Num(Mathf.Max(0.5f, material.GetFloat("_OutlineWidth") * padRatio * size * scale));
                        n["strokeAlign"] = "OUTSIDE";
                    }
                }
                bool underlay = material.IsKeywordEnabled("UNDERLAY_ON") || material.IsKeywordEnabled("UNDERLAY_INNER");
                if (underlay && material.HasProperty("_UnderlayColor"))
                {
                    float unit = padRatio * size * scale;
                    effects = new JArray(new JObject
                    {
                        ["type"] = "DROP_SHADOW",
                        ["x"] = Num(material.GetFloat("_UnderlayOffsetX") * unit),
                        ["y"] = Num(-material.GetFloat("_UnderlayOffsetY") * unit),
                        ["blur"] = Num(material.GetFloat("_UnderlaySoftness") * unit * 2f),
                        ["spread"] = Num(material.GetFloat("_UnderlayDilate") * unit * 0.5f),
                        ["color"] = Hex(material.GetColor("_UnderlayColor"))
                    });
                }
            }
            if (effects == null)
            {
                effects = Effects(t, scale);
            }
            if (effects != null)
            {
                n["effects"] = effects;
            }
            return n;
        }

        JArray Effects(Transform t, float scale)
        {
            var list = new JArray();
            foreach (var shadow in t.GetComponents<Shadow>())
            {
                if ((shadow == null) || !shadow.enabled)
                {
                    continue;
                }
                var d = shadow.effectDistance * scale;
                if (shadow is Outline)
                {
                    // Unity draws an Outline as four hard copies offset diagonally.
                    foreach (var sign in new[] { new Vector2(1, 1), new Vector2(-1, 1), new Vector2(1, -1), new Vector2(-1, -1) })
                    {
                        list.Add(new JObject { ["type"] = "DROP_SHADOW", ["x"] = Num(sign.x * d.x), ["y"] = Num(-sign.y * d.y), ["blur"] = 0, ["spread"] = 0, ["color"] = Hex(shadow.effectColor) });
                    }
                }
                else
                {
                    list.Add(new JObject { ["type"] = "DROP_SHADOW", ["x"] = Num(d.x), ["y"] = Num(-d.y), ["blur"] = 0, ["spread"] = 0, ["color"] = Hex(shadow.effectColor) });
                }
            }
            CustomEffects(t, scale, list);
            if (list.Count == 0)
            {
                return null;
            }
            return list;
        }

        /// <summary>What the layer is in Unity: hierarchy path, components, prefab, button actions. Never drawn.</summary>
        static JObject Meta(Transform t)
        {
            var unity = new JObject { ["path"] = PathOf(t) };
            var components = new JArray();
            foreach (var c in t.GetComponents<Component>())
            {
                if (c == null)
                {
                    components.Add("Missing script");
                }
                else if (!(c is Transform) && !(c is CanvasRenderer))
                {
                    components.Add(c.GetType().Name);
                }
            }
            if (components.Count > 0)
            {
                unity["components"] = components;
            }
            var go = t.gameObject;
            if (PrefabUtility.IsAnyPrefabInstanceRoot(go))
            {
                string prefab = PrefabUtility.GetPrefabAssetPathOfNearestInstanceRoot(go);
                if (!string.IsNullOrEmpty(prefab))
                {
                    unity["prefab"] = prefab;
                }
            }
            var button = t.GetComponent<Button>();
            if (button != null)
            {
                var calls = new JArray();
                for (int i = 0; i < button.onClick.GetPersistentEventCount(); i++)
                {
                    var target = button.onClick.GetPersistentTarget(i);
                    string targetName = "(none)";
                    if (target != null)
                    {
                        targetName = target.name;
                        if (target is Component tc)
                        {
                            targetName = tc.gameObject.name + "." + tc.GetType().Name;
                        }
                    }
                    calls.Add(targetName + "." + button.onClick.GetPersistentMethodName(i));
                }
                if (calls.Count > 0)
                {
                    unity["onClick"] = calls;
                }
            }
            return new JObject { ["unity"] = unity };
        }

        static string FillMethodName(Image.FillMethod method)
        {
            if (method == Image.FillMethod.Horizontal)
            {
                return "HORIZONTAL";
            }
            if (method == Image.FillMethod.Vertical)
            {
                return "VERTICAL";
            }
            if (method == Image.FillMethod.Radial90)
            {
                return "RADIAL_90";
            }
            if (method == Image.FillMethod.Radial180)
            {
                return "RADIAL_180";
            }
            return "RADIAL_360";
        }

        /// <summary>A short stable key per GameObject, so re-uploads update the same Flexma layer.</summary>
        string KeyOf(Transform t)
        {
            if (keys.TryGetValue(t, out var key))
            {
                return key;
            }
            string id = GlobalObjectId.GetGlobalObjectIdSlow(t.gameObject).ToString();
            if (id.StartsWith("GlobalObjectId_V1-0-"))
            {
                // Not a saved scene object: fall back to its hierarchy path and sibling indices.
                id = PathOf(t);
                for (var x = t; x != null; x = x.parent)
                {
                    id += "/" + x.GetSiblingIndex();
                }
            }
            key = "u" + Md5(System.Text.Encoding.UTF8.GetBytes(id)).Substring(0, 16);
            keys[t] = key;
            return key;
        }

        /// <summary>TMP 3.2+ (Unity 2023.2+) has textWrappingMode; older TMP only has enableWordWrapping.</summary>
        static bool WordWraps(TMP_Text tmp)
        {
            var mode = typeof(TMP_Text).GetProperty("textWrappingMode");
            if (mode != null)
            {
                string name = mode.GetValue(tmp).ToString();
                return (name == "Normal") || (name == "PreserveWhitespace");
            }
            var legacy = typeof(TMP_Text).GetProperty("enableWordWrapping");
            if (legacy != null)
            {
                return (bool)legacy.GetValue(tmp);
            }
            return true;
        }

        /// <summary>Effects the project's AddEffects hook adds for custom mesh effects or shaders.</summary>
        void CustomEffects(Transform t, float scale, JArray list)
        {
            if ((AddEffects == null) || !geoR.ContainsKey(t))
            {
                return;
            }
            var context = new LayerContext { Layer = t, Rect = geoR[t], Scale = scale, Effects = list };
            try
            {
                AddEffects(context);
            }
            catch (Exception e)
            {
                result.Warnings.Add("AddEffects hook failed for " + PathOf(t) + ": " + e.Message);
            }
        }

        /// <summary>Unity anchors as Flexma constraints (Unity y is up, Flexma y is down).</summary>
        static JToken Constraints(RectTransform rt)
        {
            string h = AnchorAxis(rt.anchorMin.x, rt.anchorMax.x, false);
            string v = AnchorAxis(rt.anchorMin.y, rt.anchorMax.y, true);
            if ((h == "MIN") && (v == "MIN"))
            {
                return null;
            }
            if ((h == "CENTER") && (v == "CENTER"))
            {
                return "CENTER";
            }
            return new JObject { ["horizontal"] = h, ["vertical"] = v };
        }

        static string AnchorAxis(float min, float max, bool vertical)
        {
            if (Mathf.Abs(min - max) < 0.001f)
            {
                if (min < 0.001f)
                {
                    if (vertical)
                    {
                        return "MAX";
                    }
                    return "MIN";
                }
                if (min > 0.999f)
                {
                    if (vertical)
                    {
                        return "MIN";
                    }
                    return "MAX";
                }
                if (Mathf.Abs(min - 0.5f) < 0.001f)
                {
                    return "CENTER";
                }
                return "SCALE";
            }
            if ((min < 0.001f) && (max > 0.999f))
            {
                return "STRETCH";
            }
            return "SCALE";
        }

        // ── images & fonts ───────────────────────────────────────

        ImageEntry ExportImage(Texture tex, Rect region, string uid, string name)
        {
            if (imagesByUid.TryGetValue(uid, out var known))
            {
                return known;
            }
            region.xMin = Mathf.Clamp(region.xMin, 0, tex.width);
            region.xMax = Mathf.Clamp(region.xMax, 0, tex.width);
            region.yMin = Mathf.Clamp(region.yMin, 0, tex.height);
            region.yMax = Mathf.Clamp(region.yMax, 0, tex.height);
            int w = Mathf.Max(1, Mathf.RoundToInt(region.width));
            int h = Mathf.Max(1, Mathf.RoundToInt(region.height));
            float k = 1f;
            if (Mathf.Max(w, h) > opt.MaxImageSize)
            {
                k = opt.MaxImageSize / (float)Mathf.Max(w, h);
            }

            // Blit through the GPU so non-readable and compressed textures work too.
            bool srgb = GraphicsFormatUtility.IsSRGBFormat(tex.graphicsFormat);
            var readWrite = RenderTextureReadWrite.Linear;
            if (srgb)
            {
                readWrite = RenderTextureReadWrite.sRGB;
            }
            var previous = RenderTexture.active;
            var full = RenderTexture.GetTemporary(tex.width, tex.height, 0, RenderTextureFormat.ARGB32, readWrite);
            Graphics.Blit(tex, full);
            RenderTexture.active = full;
            var pixels = new Texture2D(w, h, TextureFormat.RGBA32, false, !srgb);
            pixels.ReadPixels(new Rect(Mathf.Round(region.x), Mathf.Round(region.y), w, h), 0, 0);
            pixels.Apply();
            RenderTexture.active = previous;
            RenderTexture.ReleaseTemporary(full);
            if (k < 1f)
            {
                int nw = Mathf.Max(1, Mathf.RoundToInt(w * k));
                int nh = Mathf.Max(1, Mathf.RoundToInt(h * k));
                var small = RenderTexture.GetTemporary(nw, nh, 0, RenderTextureFormat.ARGB32, readWrite);
                Graphics.Blit(pixels, small);
                RenderTexture.active = small;
                var resized = new Texture2D(nw, nh, TextureFormat.RGBA32, false, !srgb);
                resized.ReadPixels(new Rect(0, 0, nw, nh), 0, 0);
                resized.Apply();
                RenderTexture.active = previous;
                RenderTexture.ReleaseTemporary(small);
                UnityEngine.Object.DestroyImmediate(pixels);
                pixels = resized;
                w = nw;
                h = nh;
            }
            bool opaque = pixels.GetPixels32().All(p => p.a >= 250);
            bool jpg = opaque && ((w * h) > 250000);
            byte[] bytes;
            if (jpg)
            {
                bytes = pixels.EncodeToJPG(90);
            }
            else
            {
                bytes = pixels.EncodeToPNG();
            }
            UnityEngine.Object.DestroyImmediate(pixels);

            string hash = Md5(bytes);
            if (imagesByHash.TryGetValue(hash, out var same))
            {
                imagesByUid[uid] = same;
                return same;
            }
            var entry = new ImageEntry
            {
                Key = "S" + images.Count, Name = name, Source = AssetDatabase.GetAssetPath(tex), Hash = hash,
                Width = w, Height = h, Scale = k
            };
            entry.File = entry.Key + ".png";
            if (jpg)
            {
                entry.File = entry.Key + ".jpg";
            }
            File.WriteAllBytes(Path.Combine(imageDir, entry.File), bytes);
            images.Add(entry);
            imagesByUid[uid] = entry;
            imagesByHash[hash] = entry;
            return entry;
        }

        string FontKey(UnityEngine.Object font)
        {
            if (font == null)
            {
                return null;
            }
            if (fonts.TryGetValue(font, out var known))
            {
                return known.Key;
            }
            var entry = new FontEntry { Key = "F" + fonts.Count, Asset = font.name, Family = font.name, Style = "", Source = "" };
            if (font is TMP_FontAsset fa)
            {
                entry.Family = fa.faceInfo.familyName;
                entry.Style = fa.faceInfo.styleName;
                var src = new SerializedObject(fa).FindProperty("m_SourceFontFile_EditorRef");
                if ((src != null) && (src.objectReferenceValue != null))
                {
                    entry.Source = AssetDatabase.GetAssetPath(src.objectReferenceValue);
                }
                if (string.IsNullOrEmpty(entry.Source) && !string.IsNullOrEmpty(fa.creationSettings.sourceFontFileGUID))
                {
                    entry.Source = AssetDatabase.GUIDToAssetPath(fa.creationSettings.sourceFontFileGUID);
                }
            }
            else if (font is Font f)
            {
                if ((f.fontNames != null) && (f.fontNames.Length > 0))
                {
                    entry.Family = f.fontNames[0];
                }
                entry.Source = AssetDatabase.GetAssetPath(f);
            }
            if (!entry.Source.StartsWith("Assets/"))
            {
                entry.Source = "";
            }
            fonts[font] = entry;
            return entry.Key;
        }

        // ── page layout & output ─────────────────────────────────

        /// <summary>A title per canvas, then its frames in a grid. Keys are unique in the file: scene + frame name.</summary>
        JArray LayoutPage(string sceneName, List<FrameEntry> frames)
        {
            var nodes = new JArray();
            float y = 0;
            foreach (var canvas in frames.Select(f => f.Canvas).Distinct().ToList())
            {
                var group = frames.Where(f => f.Canvas == canvas).ToList();
                float w = group.Max(f => f.Width);
                float h = group.Max(f => f.Height);
                string count = group.Count + " screens";
                if (group.Count == 1)
                {
                    count = "1 screen";
                }
                nodes.Add(new JObject
                {
                    ["type"] = "text", ["key"] = "unity:" + sceneName + "/" + canvas + " (title)",
                    ["name"] = canvas + " (title)", ["x"] = 0, ["y"] = Num(y),
                    ["text"] = canvas + "  —  " + count, ["size"] = 96, ["weight"] = 700, ["color"] = "#111111"
                });
                y += 180;
                int cols = 8;
                if (w >= 1000)
                {
                    cols = 5;
                }
                for (int i = 0; i < group.Count; i++)
                {
                    var f = group[i];
                    nodes.Add(new JObject
                    {
                        ["type"] = "frame", ["key"] = "unity:" + sceneName + "/" + f.Name, ["name"] = f.Name,
                        ["x"] = Num((i % cols) * (w + 160)), ["y"] = Num(y + ((i / cols) * (h + 200))),
                        ["w"] = Num(f.Width), ["h"] = Num(f.Height),
                        ["fill"] = opt.FrameBackground, ["clip"] = true,
                        ["description"] = f.Description,
                        ["meta"] = f.Meta,
                        ["children"] = f.Nodes
                    });
                }
                y += ((((group.Count - 1) / cols) + 1) * (h + 200)) + 300;
            }
            return nodes;
        }

        // ── prefabs -> components ────────────────────────────────

        sealed class Occurrence { public JObject Node; public string Prefab; public string Shape; }

        sealed class SameObject : IEqualityComparer<JObject>
        {
            public bool Equals(JObject a, JObject b) { return ReferenceEquals(a, b); }
            public int GetHashCode(JObject o) { return System.Runtime.CompilerServices.RuntimeHelpers.GetHashCode(o); }
        }

        static readonly string[] InstanceGeometry = { "x", "y", "w", "h", "rotation", "flipY", "constraints", "visible", "opacity", "absolute" };
        static readonly string[] NotOverridable = { "children", "key", "meta", "type", "name" };

        /// <summary>Prefabs used two or more times (same layer structure) become components on a "Components" page;
        /// each use becomes an instance whose differences are overrides addressed by the component layer's key.</summary>
        void Componentize()
        {
            var occurrences = new List<Occurrence>();
            foreach (JObject page in pages)
            {
                foreach (JObject top in (JArray)page["nodes"])
                {
                    if ((string)top["type"] == "frame")
                    {
                        CollectOccurrences(top, occurrences);
                    }
                }
            }

            // Per prefab, the most common structure wins; it needs at least two uses.
            var groups = new Dictionary<string, List<Occurrence>>();
            foreach (var byPrefab in occurrences.GroupBy(o => o.Prefab))
            {
                var best = byPrefab.GroupBy(o => o.Shape).OrderByDescending(g => g.Count()).First().ToList();
                if (best.Count >= 2)
                {
                    groups[byPrefab.Key] = best;
                }
            }

            // Keep it flat: a prefab whose uses contain another chosen prefab stays as plain layers.
            bool changed = true;
            while (changed)
            {
                changed = false;
                var members = new HashSet<JObject>(groups.Values.SelectMany(g => g.Select(o => o.Node)), new SameObject());
                foreach (var prefab in groups.Keys.ToList())
                {
                    bool nested = groups[prefab].Any(o => o.Node.Descendants().OfType<JObject>().Any(d => members.Contains(d)));
                    if (nested)
                    {
                        groups.Remove(prefab);
                        changed = true;
                    }
                }
            }
            if (groups.Count == 0)
            {
                return;
            }

            var components = new JArray();
            var usedNames = new HashSet<string>();
            float x = 0;
            float y = 0;
            float rowHeight = 0;
            foreach (var kv in groups.OrderBy(g => g.Key))
            {
                var first = kv.Value[0].Node;
                string name = Path.GetFileNameWithoutExtension(kv.Key);
                string unique = name;
                for (int dup = 2; usedNames.Contains(unique); dup++)
                {
                    unique = name + " (" + dup + ")";
                }
                usedNames.Add(unique);

                var component = (JObject)first.DeepClone();
                component["type"] = "component";
                component["name"] = unique;
                component["key"] = "unity:component/" + kv.Key;
                foreach (var prop in InstanceGeometry)
                {
                    if ((prop != "w") && (prop != "h"))
                    {
                        component.Remove(prop);
                    }
                }
                float w = (float)first["w"];
                float h = (float)first["h"];
                if ((x > 0) && ((x + w) > 4000))
                {
                    x = 0;
                    y += rowHeight + 120;
                    rowHeight = 0;
                }
                component["x"] = Num(x);
                component["y"] = Num(y);
                x += w + 120;
                rowHeight = Mathf.Max(rowHeight, h);
                components.Add(component);

                foreach (var use in kv.Value)
                {
                    use.Node.Replace(Instance(use.Node, component, (string)component["key"]));
                }
            }
            pages.Insert(0, new JObject { ["page"] = "Components", ["nodes"] = components });
        }

        static void CollectOccurrences(JObject n, List<Occurrence> list)
        {
            var children = n["children"] as JArray;
            if (children == null)
            {
                return;
            }
            foreach (JObject child in children)
            {
                var prefab = (string)child.SelectToken("meta.unity.prefab");
                if ((prefab != null) && ((string)child["type"] == "frame"))
                {
                    list.Add(new Occurrence { Node = child, Prefab = prefab, Shape = Shape(child, true) });
                }
                CollectOccurrences(child, list);
            }
        }

        /// <summary>Layer structure: types and (below the root) names, so uses with the same layers line up.</summary>
        static string Shape(JObject n, bool root)
        {
            string s = (string)n["type"];
            if (!root)
            {
                s = (string)n["name"] + ":" + s;
            }
            var children = n["children"] as JArray;
            if (children != null)
            {
                s += "[" + string.Join(",", children.Select(c => Shape((JObject)c, false))) + "]";
            }
            return s;
        }

        /// <summary>An instance of the component placed like this use, with this use's differences as overrides.</summary>
        static JObject Instance(JObject use, JObject component, string componentKey)
        {
            var instance = new JObject { ["type"] = "instance", ["key"] = use["key"], ["name"] = use["name"], ["component"] = componentKey };
            foreach (var prop in InstanceGeometry)
            {
                if (use[prop] != null)
                {
                    instance[prop] = use[prop];
                }
            }
            // Root differences (fill, clip, effects...) are set on the instance itself.
            foreach (var p in use.Properties())
            {
                if (InstanceGeometry.Contains(p.Name) || NotOverridable.Contains(p.Name))
                {
                    continue;
                }
                if (!JToken.DeepEquals(p.Value, component[p.Name]))
                {
                    instance[p.Name] = p.Value;
                }
            }
            var overrides = new JObject();
            AddOverrides((JArray)component["children"], (JArray)use["children"], overrides);
            if (overrides.Count > 0)
            {
                instance["overrides"] = overrides;
            }
            if (use["meta"] != null)
            {
                instance["meta"] = use["meta"];
            }
            return instance;
        }

        static void AddOverrides(JArray componentLayers, JArray useLayers, JObject overrides)
        {
            if ((componentLayers == null) || (useLayers == null))
            {
                return;
            }
            for (int i = 0; i < componentLayers.Count; i++)
            {
                var c = (JObject)componentLayers[i];
                var u = (JObject)useLayers[i];
                var diff = new JObject();
                foreach (var p in u.Properties())
                {
                    if (!NotOverridable.Contains(p.Name) && !JToken.DeepEquals(p.Value, c[p.Name]))
                    {
                        diff[p.Name] = p.Value;
                    }
                }
                foreach (var p in c.Properties())
                {
                    if (NotOverridable.Contains(p.Name) || (u[p.Name] != null))
                    {
                        continue;
                    }
                    var reset = DefaultValue(p.Name);
                    if (reset != null)
                    {
                        diff[p.Name] = reset;
                    }
                }
                if (diff.Count > 0)
                {
                    overrides[(string)c["key"]] = diff;
                }
                AddOverrides(c["children"] as JArray, u["children"] as JArray, overrides);
            }
        }

        /// <summary>The value a property has when a spec leaves it out, to undo a component value in an override.</summary>
        static JToken DefaultValue(string prop)
        {
            switch (prop)
            {
                case "visible": return true;
                case "opacity": return 1;
                case "rotation": return 0;
                case "flipY": return false;
                case "absolute": return false;
                case "italic": return false;
                case "letterSpacing": return 0;
                case "weight": return 400;
                case "description": return "";
                case "effects": return new JArray();
                case "hideFill": return false;
                case "wrap": return true;
                case "overflow": return "VISIBLE";
                default: return null;
            }
        }

        void WriteOutput(string folder)
        {
            if (opt.Components)
            {
                Componentize();
            }
            File.WriteAllText(Path.Combine(folder, PagesFile), pages.ToString(Formatting.None));
            var imageList = new JArray(images.Select(i => new JObject
            {
                ["key"] = i.Key, ["file"] = i.File, ["name"] = i.Name, ["src"] = i.Source, ["hash"] = i.Hash, ["w"] = i.Width, ["h"] = i.Height
            }));
            File.WriteAllText(Path.Combine(folder, ImagesFile), imageList.ToString(Formatting.Indented));
            var fontList = new JArray(fonts.Values.Select(f => new JObject
            {
                ["key"] = f.Key, ["asset"] = f.Asset, ["family"] = f.Family, ["style"] = f.Style, ["src"] = f.Source
            }));
            File.WriteAllText(Path.Combine(folder, FontsFile), fontList.ToString(Formatting.Indented));
            File.WriteAllLines(Path.Combine(folder, "excluded.txt"), excluded);

            result.Pages = pages.Count;
            result.Images = images.Count;
            result.Fonts = fonts.Count;
            result.Excluded = excluded.Count;
            File.WriteAllText(Path.Combine(folder, "summary.txt"), result.ToString());
        }

        // ── helpers ──────────────────────────────────────────────

        Color Col(Graphic g)
        {
            if (tint.TryGetValue(g, out var t))
            {
                return g.color * t;
            }
            return g.color;
        }

        static void Add(JArray list, JObject node)
        {
            if (node != null)
            {
                list.Add(node);
            }
        }

        static void Merge(JObject target, JObject extra)
        {
            foreach (var p in extra.Properties())
            {
                target[p.Name] = p.Value;
            }
        }

        /// <summary>Rounded to 0.1 px; whole numbers are written without a decimal point.</summary>
        static JToken Num(float v)
        {
            double r = Math.Round(v, 1);
            if (Math.Abs(r) < 0.05)
            {
                return 0;
            }
            if (Math.Abs(r - Math.Round(r)) < 1e-6)
            {
                return (long)Math.Round(r);
            }
            return r;
        }

        static bool IsWhite(Color c)
        {
            return (c.r > 0.996f) && (c.g > 0.996f) && (c.b > 0.996f) && (c.a > 0.996f);
        }

        static string Hex(Color c)
        {
            c = new Color(Mathf.Clamp01(c.r), Mathf.Clamp01(c.g), Mathf.Clamp01(c.b), Mathf.Clamp01(c.a));
            if (c.a > 0.996f)
            {
                return "#" + ColorUtility.ToHtmlStringRGB(c).ToLowerInvariant();
            }
            return "#" + ColorUtility.ToHtmlStringRGBA(c).ToLowerInvariant();
        }

        static string PathOf(Transform t)
        {
            string s = t.name;
            for (var x = t.parent; x != null; x = x.parent)
            {
                s = x.name + "/" + s;
            }
            return s;
        }

        static string Md5(byte[] bytes)
        {
            using (var md5 = MD5.Create())
            {
                return BitConverter.ToString(md5.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant();
            }
        }
    }
}
