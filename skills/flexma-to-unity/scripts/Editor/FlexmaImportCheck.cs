using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using Newtonsoft.Json.Linq;
using TMPro;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.SceneManagement;
using UnityEngine.UI;

namespace Flexma.Import
{
    /// <summary>
    /// Renders imported prefabs in Unity at their frame's size and compares them pixel by pixel with Flexma's own
    /// render of the frame (refs/ in the export), so an import is checked, not assumed.
    /// </summary>
    public static class FlexmaImportCheck
    {
        /// <summary>Pixels whose color differs by more than this (0–255, any channel) count as different.</summary>
        public const int Tolerance = 40;

        static readonly Color Backdrop = new Color(0.24f, 0.26f, 0.30f, 1);

        public sealed class Report
        {
            public string Frame;
            /// <summary>Share of pixels within <see cref="Tolerance"/>.</summary>
            public float Matching;
            /// <summary>Flexma, Unity and the difference side by side.</summary>
            public string Image;
            /// <summary>Where the differences are, with the layer there.</summary>
            public readonly List<string> Spots = new List<string>();

            public override string ToString() =>
                Frame + ": " + (Matching * 100).ToString("0.0") + "% of pixels match (" + Image + ")" + (Spots.Count > 0 ? "\n  differs at: " + string.Join("; ", Spots) : "");
        }

        /// <summary>Checks every frame of an import.</summary>
        public static List<Report> Check(FlexmaImporter.Result import)
        {
            var reports = new List<Report>();
            var frames = import.Manifest["frames"].Cast<JObject>().ToList();
            bool linear = (bool?)import.Manifest["linearBlending"] ?? false;
            for (int i = 0; i < frames.Count && i < import.Prefabs.Count; i++)
            {
                string refFile = (string)import.Manifest["refs"]?[(string)frames[i]["id"]];
                if (refFile == null)
                {
                    continue;
                }
                reports.Add(Check(import.Prefabs[i], Path.Combine(import.ExportDir, refFile), linear, Path.Combine(import.ExportDir, "check")));
            }
            return reports;
        }

        /// <summary>Renders `prefabPath` at the size of `refPng` and compares the two.</summary>
        public static Report Check(string prefabPath, string refPng, bool linearDesign, string outDir)
        {
            var reference = new Texture2D(2, 2, TextureFormat.RGBA32, false);
            reference.LoadImage(File.ReadAllBytes(refPng));
            int w = reference.width, h = reference.height;
            var report = new Report { Frame = Path.GetFileNameWithoutExtension(prefabPath) };
            var nodes = new List<(string name, Rect rect, int depth)>();
            var unity = Render(prefabPath, w, h, nodes);
            var a = Flatten(reference.GetPixels32(), linearDesign);
            var b = unity.GetPixels32();
            var diff = new Color32[a.Length];
            int different = 0;
            const int Cell = 64;
            int cols = (w + Cell - 1) / Cell, rows = (h + Cell - 1) / Cell;
            var cells = new int[cols * rows];
            for (int i = 0; i < a.Length; i++)
            {
                int d = Mathf.Max(Mathf.Abs(a[i].r - b[i].r), Mathf.Max(Mathf.Abs(a[i].g - b[i].g), Mathf.Abs(a[i].b - b[i].b)));
                byte grey = (byte)((b[i].r + b[i].g + b[i].b) / 3 * 0.4f);
                if (d > Tolerance)
                {
                    different++;
                    // Texture rows run bottom-up.
                    int x = i % w, y = h - 1 - i / w;
                    cells[(y / Cell) * cols + x / Cell]++;
                    diff[i] = new Color32(255, (byte)(grey / 2), (byte)(grey / 2), 255);
                }
                else
                {
                    diff[i] = new Color32(grey, grey, grey, 255);
                }
            }
            report.Matching = 1 - different / (float)a.Length;
            foreach (var c in Enumerable.Range(0, cells.Length).Where(c => cells[c] > Cell * Cell / 12).OrderByDescending(c => cells[c]).Take(8))
            {
                int cx = (c % cols) * Cell + Cell / 2, cy = (c / cols) * Cell + Cell / 2;
                var layer = nodes.Where(n => n.rect.Contains(new Vector2(cx, cy))).OrderByDescending(n => n.depth).Select(n => n.name).FirstOrDefault() ?? "nothing";
                report.Spots.Add("(" + cx + ", " + cy + ") " + layer + " " + (cells[c] * 100 / (Cell * Cell)) + "%");
            }
            Directory.CreateDirectory(outDir);
            report.Image = Path.Combine(outDir, FlexmaImporter.SafeName(report.Frame) + ".png");
            File.WriteAllBytes(report.Image, SideBySide(a, b, diff, w, h));
            Object.DestroyImmediate(reference);
            Object.DestroyImmediate(unity);
            return report;
        }

        /// <summary>Flexma's render (straight alpha) over the backdrop, blended like the design page blends.</summary>
        static Color32[] Flatten(Color32[] px, bool linear)
        {
            var bg = (Color32)Backdrop;
            var outPx = new Color32[px.Length];
            for (int i = 0; i < px.Length; i++)
            {
                float t = px[i].a / 255f;
                byte Mix(byte f, byte g)
                {
                    if (!linear)
                    {
                        return (byte)Mathf.RoundToInt(f * t + g * (1 - t));
                    }
                    float lf = Mathf.GammaToLinearSpace(f / 255f), lg = Mathf.GammaToLinearSpace(g / 255f);
                    return (byte)Mathf.RoundToInt(Mathf.LinearToGammaSpace(lf * t + lg * (1 - t)) * 255);
                }
                outPx[i] = new Color32(Mix(px[i].r, bg.r), Mix(px[i].g, bg.g), Mix(px[i].b, bg.b), 255);
            }
            return outPx;
        }

        /// <summary>The prefab drawn on a camera canvas at w × h, in a scene of its own; collects each layer's
        /// rect (pixels from the top left) for naming the differences.</summary>
        static Texture2D Render(string prefabPath, int w, int h, List<(string, Rect, int)> nodes)
        {
            var active = SceneManager.GetActiveScene();
            var scratch = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Additive);
            SceneManager.SetActiveScene(scratch);
            var scene = EditorSceneManager.NewPreviewScene();
            var rt = new RenderTexture(w, h, 24);
            GameObject camGo = null, canvasGo = null;
            try
            {
                camGo = new GameObject("Flexma check camera");
                SceneManager.MoveGameObjectToScene(camGo, scene);
                var cam = camGo.AddComponent<Camera>();
                cam.enabled = false;
                cam.scene = scene;
                cam.clearFlags = CameraClearFlags.SolidColor;
                cam.backgroundColor = Backdrop;
                cam.orthographic = true;
                cam.targetTexture = rt;
                canvasGo = new GameObject("Flexma check canvas", typeof(RectTransform));
                SceneManager.MoveGameObjectToScene(canvasGo, scene);
                var canvas = canvasGo.AddComponent<Canvas>();
                canvas.renderMode = RenderMode.ScreenSpaceCamera;
                canvas.worldCamera = cam;
                canvas.planeDistance = 1;
                // Colors near black stay exact (Unity otherwise rounds UI vertex colors in linear light).
                typeof(Canvas).GetProperty("vertexColorAlwaysGammaSpace", BindingFlags.Public | BindingFlags.Instance)?.SetValue(canvas, true);
                var scaler = canvasGo.AddComponent<CanvasScaler>();
                scaler.uiScaleMode = CanvasScaler.ScaleMode.ConstantPixelSize;
                scaler.scaleFactor = 1;
                var prefab = AssetDatabase.LoadAssetAtPath<GameObject>(prefabPath);
                var screen = (GameObject)PrefabUtility.InstantiatePrefab(prefab, scene);
                screen.transform.SetParent(canvasGo.transform, false);
                var srt = (RectTransform)screen.transform;
                if (srt.anchorMin == srt.anchorMax)
                {
                    srt.anchorMin = srt.anchorMax = srt.pivot = new Vector2(0, 1);
                    srt.anchoredPosition = Vector2.zero;
                }
                for (int i = 0; i < 3; i++)
                {
                    Canvas.ForceUpdateCanvases();
                    LayoutRebuilder.ForceRebuildLayoutImmediate((RectTransform)canvasGo.transform);
                    foreach (var t in screen.GetComponentsInChildren<TMP_Text>())
                    {
                        t.ForceMeshUpdate();
                    }
                    cam.Render();
                }
                var corners = new Vector3[4];
                foreach (var n in screen.GetComponentsInChildren<FlexmaNode>())
                {
                    ((RectTransform)n.transform).GetWorldCorners(corners);
                    Vector2 lo = cam.WorldToScreenPoint(corners[0]), hi = cam.WorldToScreenPoint(corners[2]);
                    int depth = 0;
                    for (var p = n.transform; p != null; p = p.parent)
                    {
                        depth++;
                    }
                    nodes.Add((n.name, Rect.MinMaxRect(Mathf.Min(lo.x, hi.x), h - Mathf.Max(lo.y, hi.y), Mathf.Max(lo.x, hi.x), h - Mathf.Min(lo.y, hi.y)), depth));
                }
                var prev = RenderTexture.active;
                RenderTexture.active = rt;
                var tex = new Texture2D(w, h, TextureFormat.RGBA32, false);
                tex.ReadPixels(new Rect(0, 0, w, h), 0, 0);
                tex.Apply();
                RenderTexture.active = prev;
                return tex;
            }
            finally
            {
                if (camGo != null)
                {
                    camGo.GetComponent<Camera>().targetTexture = null;
                }
                rt.Release();
                Object.DestroyImmediate(rt);
                EditorSceneManager.ClosePreviewScene(scene);
                SceneManager.SetActiveScene(active);
                EditorSceneManager.CloseScene(scratch, true);
            }
        }

        /// <summary>Flexma | Unity | difference, at half size.</summary>
        static byte[] SideBySide(Color32[] a, Color32[] b, Color32[] diff, int w, int h)
        {
            int hw = Mathf.Max(1, w / 2), hh = Mathf.Max(1, h / 2), gap = 8;
            var outTex = new Texture2D(hw * 3 + gap * 2, hh, TextureFormat.RGBA32, false);
            var px = new Color32[outTex.width * hh];
            for (int i = 0; i < px.Length; i++)
            {
                px[i] = new Color32(255, 255, 255, 255);
            }
            var sources = new[] { a, b, diff };
            for (int k = 0; k < 3; k++)
            {
                for (int y = 0; y < hh; y++)
                {
                    for (int x = 0; x < hw; x++)
                    {
                        int sx = x * 2, sy = y * 2;
                        Color32 Avg()
                        {
                            int r = 0, g = 0, bl = 0;
                            for (int dy = 0; dy < 2; dy++)
                            {
                                for (int dx = 0; dx < 2; dx++)
                                {
                                    var c = sources[k][Mathf.Min(h - 1, sy + dy) * w + Mathf.Min(w - 1, sx + dx)];
                                    r += c.r;
                                    g += c.g;
                                    bl += c.b;
                                }
                            }
                            return new Color32((byte)(r / 4), (byte)(g / 4), (byte)(bl / 4), 255);
                        }
                        px[y * outTex.width + k * (hw + gap) + x] = Avg();
                    }
                }
            }
            outTex.SetPixels32(px);
            outTex.Apply();
            var png = outTex.EncodeToPNG();
            Object.DestroyImmediate(outTex);
            return png;
        }
    }
}
