using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using UnityEngine;

/// <summary>
/// Pushes a FlexmaUIExporter export folder into a Flexma file:
/// uploads new images and fonts (remembered by content hash in Library/FlexmaExport/upload-cache.json),
/// fills in the asset ids and font families, then sends each scene page through upload_nodes.
/// Layers carry keys, so Flexma updates the layers of earlier uploads in place (ids and comments survive) and keeps
/// what people added in Flexma. Frames people moved keep their position. Un-keyed copies of the same frames, made
/// before keys existed, are removed once (a Flexma version is saved first).
/// </summary>
public static class FlexmaUploader
{
    public sealed class Settings
    {
        public string ServerUrl;
        public string Token;
        public string FileId;
        public bool ReplaceUnkeyedCopies = true;
        public bool KeepFramePositions = true;
        /// <summary>In Linear color space Unity blends translucent UI in linear light; tell Flexma to match.</summary>
        public bool LinearBlending = UnityEditor.PlayerSettings.colorSpace == ColorSpace.Linear;
    }

    public sealed class Result
    {
        public int ImagesUploaded, ImagesReused, FontsUploaded, Pages, Created, Updated, Deleted, Replaced;
        public string FileUrl;
        public readonly List<string> Warnings = new List<string>();

        public override string ToString()
        {
            string s = $"{Pages} pages: {Created} layers created, {Updated} updated, {Deleted} removed; {ImagesUploaded} new images ({ImagesReused} already uploaded), {FontsUploaded} fonts uploaded, {Replaced} old un-keyed frames replaced";
            if (Warnings.Count > 0)
            {
                s += "\n- " + string.Join("\n- ", Warnings);
            }
            return s;
        }
    }

    sealed class FontFace { public string Family; public int Weight; public bool Italic; }

    // get_file_tree lines: PAGE "name" [id], then top-level layers indented by two spaces.
    // Names are printed with " replaced by '.
    static readonly Regex PageLine = new Regex("^PAGE \"([^\"]*)\" \\[([^\\]]+)\\]", RegexOptions.Multiline);
    static readonly Regex TopLevelLine = new Regex("^  [A-Z]+ \"([^\"]*)\" \\[([^\\]]+)\\]", RegexOptions.Multiline);
    static readonly Regex CreatedPage = new Regex(@"Created page (\S+?)\.?\s*$");

    /// <summary>Pulls the file id out of a Flexma link (…/file/&lt;id&gt;) or returns the text as is.</summary>
    public static string FileIdFrom(string fileOrUrl)
    {
        if (string.IsNullOrEmpty(fileOrUrl))
        {
            return "";
        }
        var m = Regex.Match(fileOrUrl, @"/file/([A-Za-z0-9]+)");
        if (m.Success)
        {
            return m.Groups[1].Value;
        }
        return fileOrUrl.Trim();
    }

    /// <summary>The server part of a Flexma link (https://host/file/&lt;id&gt; -> https://host), or "" when it isn't a link.</summary>
    public static string ServerFrom(string fileOrUrl)
    {
        if (string.IsNullOrEmpty(fileOrUrl))
        {
            return "";
        }
        var m = Regex.Match(fileOrUrl.Trim(), @"^(https?://[^/]+)/");
        if (m.Success)
        {
            return m.Groups[1].Value;
        }
        return "";
    }

    public static Result Upload(string exportFolder, Settings settings, Func<string, float, bool> progress = null)
    {
        var result = new Result { FileUrl = settings.ServerUrl.TrimEnd('/') + "/file/" + settings.FileId };
        var images = JArray.Parse(File.ReadAllText(Path.Combine(exportFolder, FlexmaUIExporter.ImagesFile)));
        var fonts = JArray.Parse(File.ReadAllText(Path.Combine(exportFolder, FlexmaUIExporter.FontsFile)));
        var pages = JArray.Parse(File.ReadAllText(Path.Combine(exportFolder, FlexmaUIExporter.PagesFile)));

        string cachePath = Path.Combine(FlexmaUIExporter.DefaultOutputFolder, "upload-cache.json");
        var cacheRoot = new JObject();
        if (File.Exists(cachePath))
        {
            cacheRoot = JObject.Parse(File.ReadAllText(cachePath));
        }
        string cacheKey = settings.ServerUrl.TrimEnd('/') + "|" + settings.FileId;
        var cache = cacheRoot[cacheKey] as JObject;
        if (cache == null)
        {
            cache = new JObject { ["images"] = new JObject(), ["fonts"] = new JObject() };
            cacheRoot[cacheKey] = cache;
        }
        var imageCache = (JObject)cache["images"];
        var fontCache = (JObject)cache["fonts"];

        using (var client = new FlexmaClient(settings.ServerUrl, settings.Token))
        {
            try
            {
                // ── images ──
                var assetIds = new Dictionary<string, string>();
                var missing = new List<JToken>();
                foreach (var img in images)
                {
                    string id = (string)imageCache[(string)img["hash"]];
                    if (id != null)
                    {
                        assetIds[(string)img["key"]] = id;
                        result.ImagesReused++;
                    }
                    else
                    {
                        missing.Add(img);
                    }
                }
                for (int start = 0; start < missing.Count; start += 100)
                {
                    var batch = missing.Skip(start).Take(100).ToList();
                    string link = client.RequestUploadLink("upload_image", new JObject { ["file_id"] = settings.FileId, ["files"] = batch.Count });
                    foreach (var img in batch)
                    {
                        if (Report(progress, "Uploading image " + (result.ImagesUploaded + 1) + " / " + missing.Count, 0.6f * (result.ImagesUploaded / (float)missing.Count)))
                        {
                            throw new OperationCanceledException("Upload cancelled");
                        }
                        string file = (string)img["file"];
                        string type = "image/png";
                        if (file.EndsWith(".jpg"))
                        {
                            type = "image/jpeg";
                        }
                        var reply = client.PostToLink(link, File.ReadAllBytes(Path.Combine(exportFolder, FlexmaUIExporter.ImagesFolder, file)), type);
                        string id = (string)reply["id"];
                        assetIds[(string)img["key"]] = id;
                        imageCache[(string)img["hash"]] = id;
                        result.ImagesUploaded++;
                    }
                    Save(cachePath, cacheRoot);
                }

                // ── fonts ──
                Report(progress, "Fonts", 0.65f);
                var faces = ResolveFonts(client, settings, fonts, fontCache, result);
                Save(cachePath, cacheRoot);

                // ── pages ──
                string tree = client.CallTool("get_file_tree", new JObject { ["file_id"] = settings.FileId, ["depth"] = 1 });
                var pageIds = new Dictionary<string, string>();
                foreach (Match m in PageLine.Matches(tree))
                {
                    if (!pageIds.ContainsKey(m.Groups[1].Value))
                    {
                        pageIds[m.Groups[1].Value] = m.Groups[2].Value;
                    }
                }

                bool versionSaved = false;
                for (int pi = 0; pi < pages.Count; pi++)
                {
                    string pageName = (string)pages[pi]["page"];
                    var nodes = (JArray)pages[pi]["nodes"];
                    Report(progress, "Uploading page " + pageName, 0.7f + (0.3f * pi / pages.Count));
                    foreach (var node in nodes)
                    {
                        Resolve(node, assetIds, faces);
                    }

                    if (!pageIds.TryGetValue(pageName.Replace('"', '\''), out string pageId))
                    {
                        string created = client.CallTool("create_page", new JObject { ["file_id"] = settings.FileId, ["name"] = pageName });
                        var m = CreatedPage.Match(created.Trim());
                        if (!m.Success)
                        {
                            throw new FlexmaException("Could not read the new page id from: " + created);
                        }
                        pageId = m.Groups[1].Value;
                    }
                    else
                    {
                        PreparePage(client, settings, pageId, nodes, result, ref versionSaved);
                    }
                    if (settings.LinearBlending)
                    {
                        var page = new JObject { ["id"] = pageId, ["linearBlending"] = true };
                        client.CallTool("update_nodes", new JObject { ["file_id"] = settings.FileId, ["updates"] = new JArray(page) });
                    }

                    string nodeLink = client.RequestUploadLink("upload_nodes", new JObject { ["file_id"] = settings.FileId, ["parent_id"] = pageId });
                    var reply = client.PostToLink(nodeLink, Encoding.UTF8.GetBytes(nodes.ToString(Formatting.None)), "application/json");
                    result.Created += Count(reply, "created");
                    result.Updated += Count(reply, "updated");
                    result.Deleted += Count(reply, "deleted");
                    result.Pages++;
                }
            }
            finally
            {
                Save(cachePath, cacheRoot);
            }
        }
        return result;
    }

    /// <summary>Looks at the page's top-level layers: keyed frames from earlier uploads keep their position on the page,
    /// and un-keyed frames with the same names as the export (made before keys existed) are removed.</summary>
    static void PreparePage(FlexmaClient client, Settings settings, string pageId, JArray nodes, Result result, ref bool versionSaved)
    {
        string pageTree = client.CallTool("get_file_tree", new JObject { ["file_id"] = settings.FileId, ["page_id"] = pageId, ["depth"] = 2 });
        var ids = TopLevelLine.Matches(pageTree).Cast<Match>().Select(m => m.Groups[2].Value).ToList();
        if (ids.Count == 0)
        {
            return;
        }
        var existing = JArray.Parse(client.CallTool("get_nodes", new JObject { ["file_id"] = settings.FileId, ["node_ids"] = new JArray(ids) }));
        var names = new HashSet<string>(nodes.Select(n => ((string)n["name"]).Replace('"', '\'')));
        var keys = new HashSet<string>();
        var unkeyedCopies = new List<string>();
        foreach (var n in existing)
        {
            // get_nodes leaves out default values, so layers people added may have no name or key.
            string key = (string)n["key"];
            string name = (string)n["name"];
            if (key != null)
            {
                keys.Add(key);
            }
            else if ((name != null) && names.Contains(name.Replace('"', '\'')))
            {
                unkeyedCopies.Add((string)n["id"]);
            }
        }

        if (settings.KeepFramePositions)
        {
            foreach (JObject node in nodes)
            {
                if (keys.Contains((string)node["key"]))
                {
                    node.Remove("x");
                    node.Remove("y");
                }
            }
        }

        if (settings.ReplaceUnkeyedCopies && (unkeyedCopies.Count > 0))
        {
            if (!versionSaved)
            {
                client.CallTool("save_version", new JObject { ["file_id"] = settings.FileId, ["label"] = "Before Unity UI re-export " + DateTime.Now.ToString("yyyy-MM-dd HH:mm") });
                versionSaved = true;
            }
            client.CallTool("delete_nodes", new JObject { ["file_id"] = settings.FileId, ["node_ids"] = new JArray(unkeyedCopies) });
            result.Replaced += unkeyedCopies.Count;
        }
    }

    static int Count(JObject reply, string field)
    {
        if (reply[field] == null)
        {
            return 0;
        }
        return (int)reply[field];
    }

    /// <summary>Uploads font files the project has (Flexma reports the family and weight it registered);
    /// fonts without a file are added from Google Fonts by family name, falling back to Inter.</summary>
    static Dictionary<string, FontFace> ResolveFonts(FlexmaClient client, Settings settings, JArray fonts, JObject fontCache, Result result)
    {
        var faces = new Dictionary<string, FontFace>();
        var toUpload = new List<KeyValuePair<JToken, string>>();
        foreach (var f in fonts)
        {
            string src = (string)f["src"];
            if (!string.IsNullOrEmpty(src) && File.Exists(src) && (src.EndsWith(".ttf", StringComparison.OrdinalIgnoreCase) || src.EndsWith(".otf", StringComparison.OrdinalIgnoreCase)))
            {
                string hash = Md5(File.ReadAllBytes(src));
                if (fontCache[hash] is JObject cached)
                {
                    faces[(string)f["key"]] = new FontFace { Family = (string)cached["family"], Weight = (int)cached["weight"], Italic = (bool)cached["italic"] };
                }
                else
                {
                    toUpload.Add(new KeyValuePair<JToken, string>(f, hash));
                }
                continue;
            }

            // No font file in the project (Unity built-in or a TMP asset without its source).
            string family = (string)f["family"];
            if (string.IsNullOrEmpty(family) || (family == "Arial") || (family == "Liberation Sans"))
            {
                family = "Arimo";
            }
            else if (family == "NotInter")
            {
                // Unity 6's default UI Toolkit font is Inter under another name.
                family = "Inter";
            }
            try
            {
                client.CallTool("add_font", new JObject { ["file_id"] = settings.FileId, ["family"] = family });
            }
            catch (FlexmaException)
            {
                result.Warnings.Add("Font '" + f["asset"] + "' (" + family + ") has no source file and is not a Google font; using Inter.");
                family = "Inter";
            }
            faces[(string)f["key"]] = new FontFace { Family = family, Weight = WeightFromStyle((string)f["style"]), Italic = false };
        }

        if (toUpload.Count > 0)
        {
            string link = client.RequestUploadLink("upload_font", new JObject { ["file_id"] = settings.FileId, ["files"] = toUpload.Count });
            foreach (var pair in toUpload)
            {
                var reply = client.PostToLink(link, File.ReadAllBytes((string)pair.Key["src"]), "application/octet-stream");
                var face = new FontFace { Family = (string)reply["family"], Weight = 400, Italic = false };
                if (reply["weight"] != null)
                {
                    face.Weight = (int)reply["weight"];
                }
                if (reply["italic"] != null)
                {
                    face.Italic = (bool)reply["italic"];
                }
                faces[(string)pair.Key["key"]] = face;
                fontCache[pair.Value] = new JObject { ["family"] = face.Family, ["weight"] = face.Weight, ["italic"] = face.Italic };
                result.FontsUploaded++;
            }
        }
        return faces;
    }

    /// <summary>Replaces the export's @S/@F placeholders with Flexma asset ids and font families.</summary>
    static void Resolve(JToken token, Dictionary<string, string> assetIds, Dictionary<string, FontFace> faces)
    {
        if (token is JObject obj)
        {
            if (((string)obj["type"] == "text") && (obj["font"] != null))
            {
                string key = ((string)obj["font"]).TrimStart('@');
                if (faces.TryGetValue(key, out var face))
                {
                    obj["font"] = face.Family;
                    int weight = face.Weight;
                    if ((int?)obj["weight"] == 700)
                    {
                        weight = Math.Min(900, Math.Max(700, face.Weight + 200));
                    }
                    if (weight == 400)
                    {
                        obj.Remove("weight");
                    }
                    else
                    {
                        obj["weight"] = weight;
                    }
                    if (face.Italic)
                    {
                        obj["italic"] = true;
                    }
                }
                else
                {
                    obj.Remove("font");
                }
            }
            var asset = obj["asset"];
            if ((asset != null) && (asset.Type == JTokenType.String) && ((string)asset).StartsWith("@"))
            {
                if (assetIds.TryGetValue(((string)asset).Substring(1), out string id))
                {
                    obj["asset"] = id;
                }
            }
            foreach (var p in obj.Properties().ToList())
            {
                if ((p.Value.Type == JTokenType.Object) || (p.Value.Type == JTokenType.Array))
                {
                    Resolve(p.Value, assetIds, faces);
                }
            }
        }
        else if (token is JArray arr)
        {
            foreach (var item in arr)
            {
                Resolve(item, assetIds, faces);
            }
        }
    }

    static int WeightFromStyle(string style)
    {
        if (string.IsNullOrEmpty(style))
        {
            return 400;
        }
        string s = style.ToLowerInvariant().Replace(" ", "").Replace("-", "");
        if (s.Contains("thin"))
        {
            return 100;
        }
        if (s.Contains("extralight") || s.Contains("ultralight"))
        {
            return 200;
        }
        if (s.Contains("light"))
        {
            return 300;
        }
        if (s.Contains("medium"))
        {
            return 500;
        }
        if (s.Contains("semibold") || s.Contains("demibold"))
        {
            return 600;
        }
        if (s.Contains("extrabold") || s.Contains("ultrabold"))
        {
            return 800;
        }
        if (s.Contains("black") || s.Contains("heavy"))
        {
            return 900;
        }
        if (s.Contains("bold"))
        {
            return 700;
        }
        return 400;
    }

    static bool Report(Func<string, float, bool> progress, string status, float t)
    {
        if (progress == null)
        {
            return false;
        }
        return progress(status, t);
    }

    static void Save(string path, JObject root)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        File.WriteAllText(path, root.ToString(Formatting.Indented));
    }

    static string Md5(byte[] bytes)
    {
        using (var md5 = MD5.Create())
        {
            return BitConverter.ToString(md5.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant();
        }
    }
}
