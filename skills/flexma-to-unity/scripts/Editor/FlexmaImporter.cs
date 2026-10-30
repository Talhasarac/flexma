using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Text.RegularExpressions;
using Newtonsoft.Json.Linq;
using TMPro;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.SceneManagement;
using UnityEngine.TextCore.LowLevel;
using UnityEngine.UI;

namespace Flexma.Import
{
    /// <summary>
    /// Builds uGUI prefabs from Flexma frames, the same as the design. Flexma draws every layer that paints
    /// something (fills, strokes, corners, shadows, icons, the clipping of its parents) into its own image, cut
    /// down to a 9-slice where it stretches; text stays live TextMeshPro text in the design's own font files;
    /// constraints become anchors and auto layout becomes <see cref="FlexmaLayout"/>. Importing again updates the
    /// prefab in place by layer id, so components and references added in Unity survive.
    /// </summary>
    public static class FlexmaImporter
    {
        public sealed class Settings
        {
            /// <summary>The file's link, https://host/file/ID.</summary>
            public string FileLink;
            /// <summary>A Flexma API token (AI Agents > Connected agents &amp; API tokens).</summary>
            public string Token;
            /// <summary>Frame ids to import: screens, or components to use as prefabs.</summary>
            public string[] Frames;
            /// <summary>Image pixels per design pixel. 2 stays sharp on 4K screens.</summary>
            public float Scale = 2;
            /// <summary>Also import the frames' own fill, which is usually a mockup of the game behind the UI.</summary>
            public bool Background;
            /// <summary>Where the prefabs, images and fonts go.</summary>
            public string Folder = "Assets/Flexma";
        }

        public sealed class Result
        {
            public readonly List<string> Prefabs = new List<string>();
            public readonly List<string> Warnings = new List<string>();
            /// <summary>Where the download was unpacked (Library/FlexmaImport/FILE).</summary>
            public string ExportDir;
            public JObject Manifest;

            public override string ToString()
            {
                var sb = new StringBuilder();
                sb.AppendLine(Prefabs.Count + " prefab(s): " + string.Join(", ", Prefabs));
                foreach (var w in Warnings.Distinct())
                {
                    sb.AppendLine("warning: " + w);
                }
                return sb.ToString().TrimEnd();
            }
        }

        /// <summary>The folder imports of a file go into.</summary>
        public static string FileFolder(Settings s) => s.Folder.TrimEnd('/') + "/" + ParseLink(s.FileLink).fileId;

        public static Result Import(Settings s)
        {
            if ((s.Frames == null) || (s.Frames.Length == 0))
            {
                throw new ArgumentException("Name the frames to import (their Flexma ids).");
            }
            var (host, fileId) = ParseLink(s.FileLink);
            var result = new Result { ExportDir = Path.Combine("Library", "FlexmaImport", fileId) };
            try
            {
                EditorUtility.DisplayProgressBar("Flexma import", "Downloading " + s.Frames.Length + " frame(s)…", 0.1f);
                var zip = Download(host, fileId, s);
                EditorUtility.DisplayProgressBar("Flexma import", "Unpacking…", 0.4f);
                if (Directory.Exists(result.ExportDir))
                {
                    Directory.Delete(result.ExportDir, true);
                }
                Extract(zip, result.ExportDir);
                result.Manifest = JObject.Parse(File.ReadAllText(Path.Combine(result.ExportDir, "manifest.json")));
                foreach (var w in result.Manifest["warnings"] ?? new JArray())
                {
                    result.Warnings.Add((string)w);
                }
                string folder = FileFolder(s);
                EditorUtility.DisplayProgressBar("Flexma import", "Importing images…", 0.5f);
                var sprites = ImportSprites(result.Manifest, result.ExportDir, folder + "/Sprites");
                EditorUtility.DisplayProgressBar("Flexma import", "Importing fonts…", 0.7f);
                var fonts = ImportFonts(result.Manifest, result.ExportDir, folder + "/Fonts");
                EditorUtility.DisplayProgressBar("Flexma import", "Building prefabs…", 0.8f);
                Directory.CreateDirectory(folder + "/Screens");
                foreach (JObject frame in result.Manifest["frames"])
                {
                    string path = folder + "/Screens/" + SafeName((string)frame["name"]) + ".prefab";
                    new Builder(sprites, fonts, result).BuildPrefab(frame, path);
                    result.Prefabs.Add(path);
                }
                AssetDatabase.SaveAssets();
            }
            finally
            {
                EditorUtility.ClearProgressBar();
            }
            return result;
        }

        // ---------------- download ----------------
        public static (string host, string fileId) ParseLink(string link)
        {
            var m = Regex.Match(link ?? "", @"^(https?://[^/]+)/file/([A-Za-z0-9_-]+)");
            if (!m.Success)
            {
                throw new ArgumentException("The file link should look like https://host/file/ID.");
            }
            return (m.Groups[1].Value, m.Groups[2].Value);
        }

        static byte[] Download(string host, string fileId, Settings s)
        {
            if (string.IsNullOrEmpty(s.Token))
            {
                throw new ArgumentException("A Flexma API token is needed (AI Agents > Connected agents & API tokens).");
            }
            using (var http = new HttpClient { Timeout = TimeSpan.FromMinutes(10) })
            {
                string url = host + "/api/files/" + fileId + "/export.unity?nodes=" + Uri.EscapeDataString(string.Join(",", s.Frames))
                    + "&scale=" + s.Scale.ToString(CultureInfo.InvariantCulture) + "&background=" + (s.Background ? "true" : "false")
                    // Linear projects blend see-through pixels differently; Flexma corrects its images for that.
                    + "&linear=" + (PlayerSettings.colorSpace == ColorSpace.Linear ? "true" : "false");
                var req = new HttpRequestMessage(HttpMethod.Get, url);
                req.Headers.Authorization = new AuthenticationHeaderValue("Bearer", s.Token);
                var res = http.SendAsync(req).GetAwaiter().GetResult();
                var body = res.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult();
                if (!res.IsSuccessStatusCode)
                {
                    throw new Exception("Flexma answered " + (int)res.StatusCode + ": " + Encoding.UTF8.GetString(body));
                }
                return body;
            }
        }

        static void Extract(byte[] zip, string dir)
        {
            string full = Path.GetFullPath(dir) + Path.DirectorySeparatorChar;
            using (var archive = new ZipArchive(new MemoryStream(zip)))
            {
                foreach (var e in archive.Entries)
                {
                    string path = Path.GetFullPath(Path.Combine(dir, e.FullName));
                    if (!path.StartsWith(full, StringComparison.Ordinal) || e.FullName.EndsWith("/"))
                    {
                        continue;
                    }
                    Directory.CreateDirectory(Path.GetDirectoryName(path));
                    using (var src = e.Open())
                    using (var dst = File.Create(path))
                    {
                        src.CopyTo(dst);
                    }
                }
            }
        }

        // ---------------- images ----------------
        /// <summary>Copies the layer images in (named by content, so each is imported once) as sprites with their 9-slice borders.</summary>
        static Dictionary<string, Sprite> ImportSprites(JObject manifest, string dir, string folder)
        {
            Directory.CreateDirectory(folder);
            var fresh = new List<(string path, JObject info)>();
            var all = new Dictionary<string, string>();
            foreach (var p in (JObject)manifest["sprites"])
            {
                string path = folder + "/" + p.Key + ".png";
                all[p.Key] = path;
                if (!File.Exists(path))
                {
                    File.Copy(Path.Combine(dir, (string)p.Value["file"]), path);
                    fresh.Add((path, (JObject)p.Value));
                }
            }
            if (fresh.Count > 0)
            {
                AssetDatabase.Refresh();
                AssetDatabase.StartAssetEditing();
                try
                {
                    foreach (var (path, info) in fresh)
                    {
                        var ti = (TextureImporter)AssetImporter.GetAtPath(path);
                        var b = (JArray)info["border"];
                        ti.textureType = TextureImporterType.Sprite;
                        ti.spriteImportMode = SpriteImportMode.Single;
                        ti.spritePixelsPerUnit = 100f * (float)info["scale"];
                        ti.spriteBorder = new Vector4((float)b[0], (float)b[1], (float)b[2], (float)b[3]);
                        ti.alphaIsTransparency = true;
                        ti.sRGBTexture = true;
                        ti.mipmapEnabled = true;
                        ti.filterMode = FilterMode.Trilinear;
                        ti.wrapMode = TextureWrapMode.Clamp;
                        ti.npotScale = TextureImporterNPOTScale.None;
                        ti.textureCompression = TextureImporterCompression.Uncompressed;
                        var ts = new TextureImporterSettings();
                        ti.ReadTextureSettings(ts);
                        ts.spriteMeshType = SpriteMeshType.FullRect;
                        ti.SetTextureSettings(ts);
                        AssetDatabase.ImportAsset(path, ImportAssetOptions.ForceUpdate);
                    }
                }
                finally
                {
                    AssetDatabase.StopAssetEditing();
                }
            }
            return all.ToDictionary(kv => kv.Key, kv => AssetDatabase.LoadAssetAtPath<Sprite>(kv.Value));
        }

        // ---------------- fonts ----------------
        /// <summary>The design's font files as TextMeshPro font assets, each family's weights linked in their
        /// weight tables so rich text weights use the real faces.</summary>
        static Dictionary<string, TMP_FontAsset> ImportFonts(JObject manifest, string dir, string folder)
        {
            Directory.CreateDirectory(folder);
            var fonts = (JArray)manifest["fonts"];
            bool copied = false;
            foreach (JObject f in fonts)
            {
                string path = folder + "/" + (string)f["file"];
                if (!File.Exists(path))
                {
                    File.Copy(Path.Combine(dir, "fonts", (string)f["file"]), path);
                    copied = true;
                }
            }
            if (copied)
            {
                AssetDatabase.Refresh();
            }
            var assets = new Dictionary<string, TMP_FontAsset>();
            foreach (JObject f in fonts)
            {
                string file = (string)f["file"];
                string assetPath = folder + "/" + Path.GetFileNameWithoutExtension(file) + " SDF.asset";
                var fa = AssetDatabase.LoadAssetAtPath<TMP_FontAsset>(assetPath);
                if (fa == null)
                {
                    var font = AssetDatabase.LoadAssetAtPath<Font>(folder + "/" + file);
                    fa = TMP_FontAsset.CreateFontAsset(font, 90, 9, GlyphRenderMode.SDFAA, 1024, 1024, AtlasPopulationMode.Dynamic, true);
                    fa.name = Path.GetFileNameWithoutExtension(assetPath);
                    AssetDatabase.CreateAsset(fa, assetPath);
                    foreach (var tex in fa.atlasTextures)
                    {
                        tex.name = fa.name + " Atlas";
                        AssetDatabase.AddObjectToAsset(tex, fa);
                    }
                    fa.material.name = fa.name + " Material";
                    AssetDatabase.AddObjectToAsset(fa.material, fa);
                }
                assets[file] = fa;
            }
            foreach (var family in fonts.GroupBy(f => (string)f["family"]))
            {
                var members = family.Select(f => (weight: (int)f["weight"], italic: (bool)f["italic"], asset: assets[(string)f["file"]])).ToList();
                foreach (var m in members)
                {
                    var table = m.asset.fontWeightTable;
                    foreach (var other in members)
                    {
                        int i = Mathf.Clamp(other.weight / 100, 0, table.Length - 1);
                        if (other.italic)
                        {
                            table[i].italicTypeface = other.asset;
                        }
                        else
                        {
                            table[i].regularTypeface = other.asset;
                        }
                    }
                    EditorUtility.SetDirty(m.asset);
                }
            }
            return assets;
        }

        // ---------------- names ----------------
        public static string SafeName(string name)
        {
            var s = Regex.Replace(name ?? "Frame", @"[\\/:*?""<>|]+", "-").Trim(' ', '.', '-');
            return s.Length == 0 ? "Frame" : s;
        }

        static float F(JToken t, float fallback = 0) => t == null || t.Type == JTokenType.Null ? fallback : (float)t;

        /// <summary>The object's component of type T, added if missing (Unity's null objects defeat `??`).</summary>
        static T Ensure<T>(GameObject go) where T : Component
        {
            var c = go.GetComponent<T>();
            return c != null ? c : go.AddComponent<T>();
        }

        static Color ParseColor(string hex)
        {
            return ColorUtility.TryParseHtmlString(hex, out var c) ? c : Color.black;
        }

        /// <summary>Builds (or updates) one frame's prefab.</summary>
        sealed class Builder
        {
            const string PaintName = "<paint>";
            readonly Dictionary<string, Sprite> sprites;
            readonly Dictionary<string, TMP_FontAsset> fonts;
            readonly Result result;
            Dictionary<string, GameObject> byId;
            readonly HashSet<GameObject> seen = new HashSet<GameObject>();

            public Builder(Dictionary<string, Sprite> sprites, Dictionary<string, TMP_FontAsset> fonts, Result result)
            {
                this.sprites = sprites;
                this.fonts = fonts;
                this.result = result;
            }

            public void BuildPrefab(JObject frame, string path)
            {
                // New objects go into a scratch scene, so the open scenes stay untouched.
                var active = SceneManager.GetActiveScene();
                var scratch = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Additive);
                SceneManager.SetActiveScene(scratch);
                bool existing = File.Exists(path);
                var root = existing ? PrefabUtility.LoadPrefabContents(path) : new GameObject((string)frame["name"], typeof(RectTransform));
                try
                {
                    byId = new Dictionary<string, GameObject>();
                    foreach (var n in root.GetComponentsInChildren<FlexmaNode>(true))
                    {
                        if (!string.IsNullOrEmpty(n.id) && !byId.ContainsKey(n.id))
                        {
                            byId[n.id] = n.gameObject;
                        }
                    }
                    Build(frame, root, null, Vector2.zero, false);
                    // Layers deleted in Flexma go (with anything added under them).
                    foreach (var n in root.GetComponentsInChildren<FlexmaNode>(true).ToList())
                    {
                        if ((n != null) && !seen.Contains(n.gameObject))
                        {
                            UnityEngine.Object.DestroyImmediate(n.gameObject);
                        }
                    }
                    PrefabUtility.SaveAsPrefabAsset(root, path);
                }
                finally
                {
                    if (existing)
                    {
                        PrefabUtility.UnloadPrefabContents(root);
                    }
                    else
                    {
                        UnityEngine.Object.DestroyImmediate(root);
                    }
                    SceneManager.SetActiveScene(active);
                    EditorSceneManager.CloseScene(scratch, true);
                }
            }

            void Build(JObject n, GameObject go, Transform parent, Vector2 parentSize, bool parentLays)
            {
                string id = (string)n["id"];
                bool root = parent == null;
                if (go == null)
                {
                    if (!byId.TryGetValue(id, out go) || (go == null))
                    {
                        go = new GameObject((string)n["name"], typeof(RectTransform));
                    }
                }
                seen.Add(go);
                if (!root)
                {
                    go.transform.SetParent(parent, false);
                }
                go.name = (string)n["name"];
                go.layer = 5; // UI
                var node = Ensure<FlexmaNode>(go);
                node.id = id;
                node.horizontal = ParseSizing((string)n["sizing"]?["h"]);
                node.vertical = ParseSizing((string)n["sizing"]?["v"]);
                node.absolute = (bool?)n["absolute"] ?? false;
                var text = n["text"] as JObject;
                string autoResize = (string)text?["autoResize"];
                node.textHugsWidth = autoResize == "WIDTH_AND_HEIGHT";
                node.textHugsHeight = (text != null) && (autoResize != "NONE");
                go.SetActive(root || ((bool?)n["visible"] ?? true));

                var size = new Vector2(F(n["w"]), F(n["h"]));
                Place((RectTransform)go.transform, n, size, parentSize, root, parentLays && !node.absolute);

                float opacity = F(n["opacity"], 1);
                var group = go.GetComponent<CanvasGroup>();
                if (opacity < 1)
                {
                    Ensure<CanvasGroup>(go).alpha = opacity;
                }
                else if (group != null)
                {
                    group.alpha = 1;
                }

                var layout = n["layout"] as JObject;
                if (text != null)
                {
                    Text(go, n, text, size);
                    node.lineHeight = F(text["lineHeight"], F(text["size"], 16) * 1.2f);
                    node.designText = go.GetComponent<TextMeshProUGUI>().text;
                    node.designSize = size;
                }
                else
                {
                    node.lineHeight = 0;
                    node.designText = null;
                    node.designSize = Vector2.zero;
                }
                Paint(go, n, size);
                Layout(go, layout);
                // Hugging frames and text outside auto layout size themselves.
                bool selfSizing = !(parentLays && !node.absolute) && !root;
                bool hugW = (layout != null && node.horizontal == Sizing.Hug) || node.textHugsWidth;
                bool hugH = (layout != null && node.vertical == Sizing.Hug) || node.textHugsHeight;
                var fitter = go.GetComponent<ContentSizeFitter>();
                if (selfSizing && (hugW || hugH))
                {
                    fitter = Ensure<ContentSizeFitter>(go);
                    fitter.horizontalFit = hugW ? ContentSizeFitter.FitMode.PreferredSize : ContentSizeFitter.FitMode.Unconstrained;
                    fitter.verticalFit = hugH ? ContentSizeFitter.FitMode.PreferredSize : ContentSizeFitter.FitMode.Unconstrained;
                }
                else if (fitter != null)
                {
                    UnityEngine.Object.DestroyImmediate(fitter);
                }

                var kids = n["children"] as JArray ?? new JArray();
                // A clipping frame whose children reach past it clips them (text and moving content too);
                // images already come cut by it.
                bool overflows = kids.OfType<JObject>().Any(c =>
                    F(c["x"]) < -0.5f || F(c["y"]) < -0.5f || F(c["x"]) + F(c["w"]) > size.x + 0.5f || F(c["y"]) + F(c["h"]) > size.y + 0.5f);
                var mask = go.GetComponent<RectMask2D>();
                if (((bool?)n["clips"] ?? false) && overflows)
                {
                    if (mask == null)
                    {
                        go.AddComponent<RectMask2D>();
                    }
                }
                else if (mask != null)
                {
                    UnityEngine.Object.DestroyImmediate(mask);
                }

                int order = go.transform.Find(PaintName) != null ? 1 : 0;
                foreach (JObject c in kids)
                {
                    var child = byId.TryGetValue((string)c["id"], out var existing) ? existing : null;
                    if (child == null)
                    {
                        child = new GameObject((string)c["name"], typeof(RectTransform));
                        byId[(string)c["id"]] = child;
                    }
                    Build(c, child, go.transform, size, layout != null);
                    child.transform.SetSiblingIndex(order++);
                }
            }

            static Sizing ParseSizing(string s) => s == "HUG" ? Sizing.Hug : s == "FILL" ? Sizing.Fill : Sizing.Fixed;

            /// <summary>The layer's rect in its parent: anchors from its constraints, like Flexma resizing frames.</summary>
            static void Place(RectTransform rt, JObject n, Vector2 size, Vector2 parentSize, bool root, bool laidOut)
            {
                float x = F(n["x"]), y = F(n["y"]);
                float rotation = F(n["rotation"]);
                bool flipX = (bool?)n["flipX"] ?? false, flipY = (bool?)n["flipY"] ?? false;
                rt.localRotation = Quaternion.Euler(0, 0, -rotation);
                rt.localScale = new Vector3(flipX ? -1 : 1, flipY ? -1 : 1, 1);
                if (root)
                {
                    // A component is a fixed-size template; a screen fills the canvas it's put on.
                    if ((string)n["type"] == "COMPONENT")
                    {
                        rt.anchorMin = rt.anchorMax = rt.pivot = new Vector2(0, 1);
                        rt.anchoredPosition = Vector2.zero;
                        rt.sizeDelta = size;
                    }
                    else
                    {
                        rt.anchorMin = Vector2.zero;
                        rt.anchorMax = Vector2.one;
                        rt.pivot = new Vector2(0.5f, 0.5f);
                        rt.offsetMin = rt.offsetMax = Vector2.zero;
                    }
                    return;
                }
                string h = (string)n["constraints"]?["h"] ?? "MIN", v = (string)n["constraints"]?["v"] ?? "MIN";
                if (laidOut)
                {
                    h = "MIN";
                    v = "MIN";
                }
                Vector2 Anchors(string c, float pos, float len, float parent, bool vertical)
                {
                    switch (c)
                    {
                        case "MAX": return vertical ? Vector2.zero : Vector2.one;
                        case "CENTER": return new Vector2(0.5f, 0.5f);
                        case "STRETCH": return new Vector2(0, 1);
                        case "SCALE":
                            if (parent <= 0)
                            {
                                return vertical ? Vector2.one : Vector2.zero;
                            }
                            return vertical ? new Vector2(1 - (pos + len) / parent, 1 - pos / parent) : new Vector2(pos / parent, (pos + len) / parent);
                        default: return vertical ? Vector2.one : Vector2.zero;
                    }
                }
                var ah = Anchors(h, x, size.x, parentSize.x, false);
                var av = Anchors(v, y, size.y, parentSize.y, true);
                // Rotated or flipped layers turn around their center, as in Flexma.
                bool turned = rotation != 0 || flipX || flipY;
                rt.pivot = turned ? new Vector2(0.5f, 0.5f) : PivotFor(h, v, n);
                rt.anchorMin = new Vector2(ah.x, av.x);
                rt.anchorMax = new Vector2(ah.y, av.y);
                // Unity measures y up from the bottom.
                float bottom = parentSize.y - y - size.y, top = parentSize.y - y;
                rt.offsetMin = new Vector2(x - ah.x * parentSize.x, bottom - av.x * parentSize.y);
                rt.offsetMax = new Vector2(x + size.x - ah.y * parentSize.x, top - av.y * parentSize.y);
            }

            /// <summary>Where a self-sizing layer stays put when its content changes size: the edge or center its
            /// constraints keep (text: its alignment).</summary>
            static Vector2 PivotFor(string h, string v, JObject n)
            {
                float px = h == "MAX" ? 1 : h == "CENTER" ? 0.5f : 0;
                float py = v == "MAX" ? 0 : v == "CENTER" ? 0.5f : 1;
                var align = (string)n["text"]?["align"];
                if ((align != null) && (h == "MIN"))
                {
                    px = align == "RIGHT" ? 1 : align == "CENTER" ? 0.5f : 0;
                }
                return new Vector2(px, py);
            }

            /// <summary>The layer's own image: on the layer itself when it covers exactly its box, otherwise on a
            /// "&lt;paint&gt;" child stretched past the box (shadows, outside strokes).</summary>
            void Paint(GameObject go, JObject n, Vector2 size)
            {
                var paintChild = go.transform.Find(PaintName);
                var own = go.GetComponent<Image>();
                var s = n["sprite"] as JObject;
                Sprite sprite = null;
                if ((s != null) && !sprites.TryGetValue((string)s["id"], out sprite))
                {
                    result.Warnings.Add("missing image for " + go.name);
                }
                if (sprite == null)
                {
                    if (paintChild != null)
                    {
                        UnityEngine.Object.DestroyImmediate(paintChild.gameObject);
                    }
                    if ((own != null) && (n["text"] == null))
                    {
                        UnityEngine.Object.DestroyImmediate(own);
                    }
                    return;
                }
                float sx = F(s["x"]), sy = F(s["y"]), sw = F(s["w"]), sh = F(s["h"]);
                bool fits = Mathf.Abs(sx) < 0.5f && Mathf.Abs(sy) < 0.5f && Mathf.Abs(sw - size.x) < 0.5f && Mathf.Abs(sh - size.y) < 0.5f;
                Image img;
                if (fits && (n["text"] == null))
                {
                    if (paintChild != null)
                    {
                        UnityEngine.Object.DestroyImmediate(paintChild.gameObject);
                    }
                    img = Ensure<Image>(go);
                }
                else
                {
                    if ((own != null) && (n["text"] == null))
                    {
                        UnityEngine.Object.DestroyImmediate(own);
                    }
                    if (paintChild == null)
                    {
                        paintChild = new GameObject(PaintName, typeof(RectTransform)).transform;
                        paintChild.SetParent(go.transform, false);
                        paintChild.gameObject.layer = 5;
                    }
                    paintChild.SetSiblingIndex(0);
                    var le = Ensure<LayoutElement>(paintChild.gameObject);
                    le.ignoreLayout = true;
                    var prt = (RectTransform)paintChild;
                    prt.anchorMin = Vector2.zero;
                    prt.anchorMax = Vector2.one;
                    prt.pivot = new Vector2(0.5f, 0.5f);
                    prt.localRotation = Quaternion.identity;
                    prt.localScale = Vector3.one;
                    prt.offsetMin = new Vector2(sx, size.y - (sy + sh));
                    prt.offsetMax = new Vector2(sx + sw - size.x, -sy);
                    img = Ensure<Image>(paintChild.gameObject);
                }
                img.sprite = sprite;
                img.type = sprite.border != Vector4.zero ? Image.Type.Sliced : Image.Type.Simple;
                img.fillCenter = true;
                img.preserveAspect = false;
                img.pixelsPerUnitMultiplier = 1;
                img.color = Color.white;
                string type = (string)n["type"];
                // Panels catch clicks like uGUI panels do; icons and decorations let them through.
                img.raycastTarget = (type == "FRAME" || type == "COMPONENT" || type == "INSTANCE" || type == "RECTANGLE") && (img.gameObject == go);
            }

            static void Layout(GameObject go, JObject layout)
            {
                var l = go.GetComponent<FlexmaLayout>();
                if (layout == null)
                {
                    if (l != null)
                    {
                        UnityEngine.Object.DestroyImmediate(l);
                    }
                    return;
                }
                l = Ensure<FlexmaLayout>(go);
                string mode = (string)layout["mode"];
                l.mode = mode == "VERTICAL" ? FlexmaLayoutMode.Vertical : mode == "GRID" ? FlexmaLayoutMode.Grid : FlexmaLayoutMode.Horizontal;
                var pad = (JArray)layout["padding"];
                l.paddingTop = F(pad[0]);
                l.paddingRight = F(pad[1]);
                l.paddingBottom = F(pad[2]);
                l.paddingLeft = F(pad[3]);
                l.gap = F(layout["gap"]);
                l.crossGap = F(layout["crossGap"], l.gap);
                l.primary = ParseAlign((string)layout["primary"]);
                l.counter = ParseAlign((string)layout["counter"]);
                l.wrap = (bool?)layout["wrap"] ?? false;
                var grid = layout["grid"] as JObject;
                if (grid != null)
                {
                    l.gridColumns = (int?)grid["columns"] ?? 0;
                    l.gridRows = (int?)grid["rows"] ?? 0;
                    var cell = grid["cell"] as JArray;
                    l.gridCell = cell != null ? new Vector2(F(cell[0]), F(cell[1])) : Vector2.zero;
                    string corner = (string)grid["startCorner"];
                    l.gridStartCorner = corner == "TOP_RIGHT" ? FlexmaGridCorner.TopRight : corner == "BOTTOM_LEFT" ? FlexmaGridCorner.BottomLeft : corner == "BOTTOM_RIGHT" ? FlexmaGridCorner.BottomRight : FlexmaGridCorner.TopLeft;
                    l.gridVertical = (bool?)grid["vertical"] ?? false;
                }
            }

            static FlexmaAlign ParseAlign(string s)
            {
                switch (s)
                {
                    case "CENTER": return FlexmaAlign.Center;
                    case "END": return FlexmaAlign.End;
                    case "SPACE_BETWEEN": return FlexmaAlign.SpaceBetween;
                    case "STRETCH": return FlexmaAlign.Stretch;
                    default: return FlexmaAlign.Start;
                }
            }

            /// <summary>TextMeshPro text set up to draw like Flexma: the same font file, size, letter spacing, line
            /// height and baseline (margins put each baseline where Flexma's line box puts it).</summary>
            void Text(GameObject go, JObject n, JObject t, Vector2 size)
            {
                var tmp = Ensure<TextMeshProUGUI>(go);
                string fontFile = (string)t["font"];
                TMP_FontAsset fa = null;
                if ((fontFile == null) || !fonts.TryGetValue(fontFile, out fa))
                {
                    result.Warnings.Add("no font file for '" + (string)t["family"] + "' in " + go.name + "; using TextMeshPro's default");
                    fa = TMP_Settings.defaultFontAsset;
                }
                tmp.font = fa;
                float fontSize = F(t["size"], 16);
                var auto = t["autoFontSize"] as JObject;
                tmp.enableAutoSizing = auto != null;
                if (auto != null)
                {
                    tmp.fontSizeMin = F(auto["min"]);
                    tmp.fontSizeMax = F(auto["max"]);
                }
                tmp.fontSize = fontSize;
                tmp.fontWeight = FontWeight.Regular; // the font file is already the weight
                float lh = F(t["lineHeight"], fontSize * 1.2f);
                tmp.characterSpacing = F(t["letterSpacing"]) / (0.01f * fontSize);
                tmp.wordSpacing = 0;
                tmp.paragraphSpacing = 0;
                if (fa != null)
                {
                    var face = fa.faceInfo;
                    float scale = fontSize / face.pointSize * (face.scale == 0 ? 1 : face.scale);
                    float asc = face.ascentLine * scale, desc = -face.descentLine * scale;
                    tmp.lineSpacing = (lh - face.lineHeight * scale) / (0.01f * fontSize);
                    // Flexma centers ascent + descent in each line box. The same margin above and below keeps the
                    // text's own height at lines × line height; the shift moves TMP's baseline onto Flexma's
                    // (whose font metrics are rounded to whole pixels, like browsers).
                    float pad = (lh - (asc + desc)) / 2;
                    float shift = (F(t["ascent"], asc) - F(t["descent"], desc)) / 2 - (asc - desc) / 2;
                    tmp.margin = new Vector4(0, pad + shift, 0, pad - shift);
                }
                string h = (string)t["align"], v = (string)t["valign"];
                var ha = h == "CENTER" ? HorizontalAlignmentOptions.Center : h == "RIGHT" ? HorizontalAlignmentOptions.Right : HorizontalAlignmentOptions.Left;
                var va = v == "CENTER" ? VerticalAlignmentOptions.Middle : v == "BOTTOM" ? VerticalAlignmentOptions.Bottom : VerticalAlignmentOptions.Top;
                tmp.horizontalAlignment = ha;
                tmp.verticalAlignment = va;
                bool wraps = ((string)t["autoResize"] != "WIDTH_AND_HEIGHT") && ((bool?)t["wrap"] ?? true);
#if UNITY_2023_2_OR_NEWER
                tmp.textWrappingMode = wraps ? TextWrappingModes.Normal : TextWrappingModes.NoWrap;
#else
                tmp.enableWordWrapping = wraps;
#endif
                switch ((string)t["overflow"])
                {
                    case "CLIP": tmp.overflowMode = TextOverflowModes.Masking; break;
                    case "ELLIPSIS": tmp.overflowMode = TextOverflowModes.Ellipsis; break;
                    case "TRUNCATE": tmp.overflowMode = TextOverflowModes.Truncate; break;
                    default: tmp.overflowMode = TextOverflowModes.Overflow; break;
                }
                var style = FontStyles.Normal;
                string deco = (string)t["decoration"];
                if (deco == "UNDERLINE")
                {
                    style |= FontStyles.Underline;
                }
                if (deco == "STRIKETHROUGH")
                {
                    style |= FontStyles.Strikethrough;
                }
                if ((bool?)t["italic"] == true && (fa == null || !fa.name.Contains("Italic")))
                {
                    style |= FontStyles.Italic;
                }
                tmp.fontStyle = style;
                tmp.color = ParseColor((string)t["color"]);
                tmp.raycastTarget = false;
                var runs = t["runs"] as JArray;
                string chars = (string)t["characters"] ?? "";
                tmp.richText = runs != null;
                tmp.text = runs != null ? RichText(chars, runs, fontSize, F(t["letterSpacing"]), go.name) : chars;
                if (t["outline"] != null)
                {
                    result.Warnings.Add("text outline on " + go.name + " isn't imported yet");
                }
                if (t["shadow"] != null)
                {
                    result.Warnings.Add("text shadow on " + go.name + " isn't imported yet");
                }
            }

            /// <summary>Flexma's text runs (character ranges with their own weight, color, spacing) as TMP rich text.</summary>
            string RichText(string chars, JArray runs, float fontSize, float baseSpacing, string layer)
            {
                // Run offsets count characters (code points), not UTF-16 units.
                var cps = new List<string>();
                for (int i = 0; i < chars.Length; i++)
                {
                    if (char.IsHighSurrogate(chars[i]) && (i + 1 < chars.Length))
                    {
                        cps.Add(chars.Substring(i, 2));
                        i++;
                    }
                    else
                    {
                        cps.Add(chars[i].ToString());
                    }
                }
                string Plain(int a, int b) => b > a ? "<noparse>" + string.Concat(cps.Skip(a).Take(b - a)) + "</noparse>" : "";
                var sb = new StringBuilder();
                int at = 0;
                foreach (JObject r in runs.OrderBy(r => (int)r["start"]))
                {
                    int start = Mathf.Clamp((int)r["start"], at, cps.Count), end = Mathf.Clamp((int)r["end"], start, cps.Count);
                    sb.Append(Plain(at, start));
                    var open = new StringBuilder();
                    var close = new List<string>();
                    if (r["weight"] != null)
                    {
                        open.Append("<font-weight=" + (int)r["weight"] + ">");
                        close.Insert(0, "</font-weight>");
                    }
                    if ((bool?)r["italic"] == true)
                    {
                        open.Append("<i>");
                        close.Insert(0, "</i>");
                    }
                    if (r["color"] != null)
                    {
                        open.Append("<color=" + (string)r["color"] + ">");
                        close.Insert(0, "</color>");
                    }
                    if (r["letterSpacing"] != null)
                    {
                        float em = (F(r["letterSpacing"]) - baseSpacing) / fontSize;
                        open.Append("<cspace=" + em.ToString("0.####", CultureInfo.InvariantCulture) + "em>");
                        close.Insert(0, "</cspace>");
                    }
                    string d = (string)r["decoration"];
                    if (d == "UNDERLINE")
                    {
                        open.Append("<u>");
                        close.Insert(0, "</u>");
                    }
                    else if (d == "STRIKETHROUGH")
                    {
                        open.Append("<s>");
                        close.Insert(0, "</s>");
                    }
                    if (r["font"] != null && r["weight"] == null)
                    {
                        result.Warnings.Add("a text run in " + layer + " uses another font family, which isn't imported yet");
                    }
                    sb.Append(open).Append(Plain(start, end)).Append(string.Concat(close));
                    at = end;
                }
                sb.Append(Plain(at, cps.Count));
                return sb.ToString();
            }
        }
    }
}
