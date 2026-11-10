using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEngine;

/// <summary>
/// Tools > Flexma > UI Exporter. Exports the uGUI canvases (and, when switched on, the UI Toolkit documents) of the
/// chosen scenes (FlexmaUIExporter) and uploads them to a Flexma file (FlexmaUploader). The API token is kept in
/// EditorPrefs on this machine only; the file link, scenes and options are remembered per project.
///
/// From code (an agent's execute-code tool): FlexmaUIExporterWindow.ExportAndUpload(true) with the saved settings, and
/// FlexmaUIExporterWindow.Include(canvases, uiToolkit) to choose what is exported.
/// Batch mode: Unity -batchmode -quit -projectPath . -executeMethod FlexmaUIExporterWindow.Batch
///   -flexmaFile https://host/file/&lt;id&gt; [-flexmaScenes Assets/A.unity,Assets/B.unity] [-flexmaExportOnly]
///   [-flexmaUIToolkit] [-flexmaNoCanvases]
///   with the token in the FLEXMA_TOKEN environment variable.
/// </summary>
public sealed class FlexmaUIExporterWindow : EditorWindow
{
    const string Prefix = "FlexmaExporter.";

    /// <summary>Everything the window remembers.</summary>
    internal sealed class Prefs
    {
        public string Token;
        public string File;
        public bool ReplaceUnkeyed;
        public bool KeepPositions;
        public bool AlphaMasks;
        public bool TextOverflow;
        public bool Components;
        public bool Canvases;
        public bool UIToolkit;
        public int MaxImageSize;
        public List<string> Scenes;

        /// <summary>EditorPrefs are shared by every project on the machine, so project settings get the project's path in their key.</summary>
        static string ProjectPrefix
        {
            get { return Prefix + Application.dataPath + "."; }
        }

        public static Prefs Load()
        {
            var p = new Prefs
            {
                Token = EditorPrefs.GetString(Prefix + "Token", ""),
                File = EditorPrefs.GetString(ProjectPrefix + "File", ""),
                ReplaceUnkeyed = EditorPrefs.GetBool(ProjectPrefix + "ReplaceUnkeyed", true),
                KeepPositions = EditorPrefs.GetBool(ProjectPrefix + "KeepPositions", true),
                AlphaMasks = EditorPrefs.GetBool(ProjectPrefix + "AlphaMasks", true),
                TextOverflow = EditorPrefs.GetBool(ProjectPrefix + "TextOverflow", true),
                Components = EditorPrefs.GetBool(ProjectPrefix + "Components", true),
                Canvases = EditorPrefs.GetBool(ProjectPrefix + "Canvases", true),
                UIToolkit = EditorPrefs.GetBool(ProjectPrefix + "UIToolkit", false),
                MaxImageSize = EditorPrefs.GetInt(ProjectPrefix + "MaxImage", 2048)
            };
            string saved = EditorPrefs.GetString(ProjectPrefix + "Scenes", "");
            if (string.IsNullOrEmpty(saved))
            {
                p.Scenes = EditorBuildSettings.scenes.Where(s => s.enabled).Select(s => s.path).ToList();
            }
            else
            {
                p.Scenes = saved.Split('|').Where(s => s.Length > 0).ToList();
            }
            return p;
        }

        public void Save()
        {
            EditorPrefs.SetString(Prefix + "Token", Token);
            EditorPrefs.SetString(ProjectPrefix + "File", File);
            EditorPrefs.SetBool(ProjectPrefix + "ReplaceUnkeyed", ReplaceUnkeyed);
            EditorPrefs.SetBool(ProjectPrefix + "KeepPositions", KeepPositions);
            EditorPrefs.SetBool(ProjectPrefix + "AlphaMasks", AlphaMasks);
            EditorPrefs.SetBool(ProjectPrefix + "TextOverflow", TextOverflow);
            EditorPrefs.SetBool(ProjectPrefix + "Components", Components);
            EditorPrefs.SetBool(ProjectPrefix + "Canvases", Canvases);
            EditorPrefs.SetBool(ProjectPrefix + "UIToolkit", UIToolkit);
            EditorPrefs.SetInt(ProjectPrefix + "MaxImage", MaxImageSize);
            EditorPrefs.SetString(ProjectPrefix + "Scenes", string.Join("|", Scenes));
        }

        public bool CanUpload
        {
            get { return !string.IsNullOrEmpty(Token) && !string.IsNullOrEmpty(FlexmaUploader.ServerFrom(File)); }
        }

        public bool ExportsSomething
        {
            get { return Canvases || UIToolkit; }
        }

        public FlexmaUIExporter.Options ExportOptions()
        {
            return new FlexmaUIExporter.Options
            {
                ScenePaths = Scenes.Where(s => !string.IsNullOrEmpty(s)).ToList(),
                OutputFolder = FlexmaUIExporter.DefaultOutputFolder,
                MaxImageSize = MaxImageSize,
                AlphaMasks = AlphaMasks,
                TextOverflow = TextOverflow,
                Components = Components,
                Canvases = Canvases,
                UIToolkit = UIToolkit
            };
        }

        public FlexmaUploader.Settings UploadSettings()
        {
            return new FlexmaUploader.Settings
            {
                ServerUrl = FlexmaUploader.ServerFrom(File),
                Token = Token,
                FileId = FlexmaUploader.FileIdFrom(File),
                ReplaceUnkeyedCopies = ReplaceUnkeyed,
                KeepFramePositions = KeepPositions
            };
        }
    }

    Prefs prefs;
    Vector2 scroll;
    string status = "";

    [MenuItem("Tools/Flexma/UI Exporter")]
    static void Open()
    {
        GetWindow<FlexmaUIExporterWindow>("Flexma UI Exporter");
    }

    void OnEnable()
    {
        prefs = Prefs.Load();
    }

    void OnGUI()
    {
        scroll = EditorGUILayout.BeginScrollView(scroll);
        EditorGUI.BeginChangeCheck();

        EditorGUILayout.LabelField("Flexma", EditorStyles.boldLabel);
        prefs.File = EditorGUILayout.TextField(new GUIContent("File link", "The Flexma file to upload into, e.g. https://your-flexma-host/file/AbC123. Remembered for this project."), prefs.File);
        prefs.Token = EditorGUILayout.PasswordField(new GUIContent("API token", "Flexma sidebar > AI Agents > Connected agents & API tokens. Stored in EditorPrefs on this machine only."), prefs.Token);
        prefs.KeepPositions = EditorGUILayout.Toggle(new GUIContent("Keep frame positions", "Frames already in Flexma stay where people put them; only new frames are placed on the grid."), prefs.KeepPositions);
        prefs.ReplaceUnkeyed = EditorGUILayout.Toggle(new GUIContent("Replace old un-keyed copies", "Removes same-named frames that were made without keys (a Flexma version is saved first). Keyed frames are always updated in place."), prefs.ReplaceUnkeyed);

        EditorGUILayout.Space();
        EditorGUILayout.LabelField("Scenes (one Flexma page each)", EditorStyles.boldLabel);
        int remove = -1;
        for (int i = 0; i < prefs.Scenes.Count; i++)
        {
            EditorGUILayout.BeginHorizontal();
            var asset = AssetDatabase.LoadAssetAtPath<SceneAsset>(prefs.Scenes[i]);
            var picked = (SceneAsset)EditorGUILayout.ObjectField(asset, typeof(SceneAsset), false);
            if (picked != asset)
            {
                prefs.Scenes[i] = AssetDatabase.GetAssetPath(picked);
            }
            if (GUILayout.Button("−", GUILayout.Width(24)))
            {
                remove = i;
            }
            EditorGUILayout.EndHorizontal();
        }
        if (remove >= 0)
        {
            prefs.Scenes.RemoveAt(remove);
        }
        var added = (SceneAsset)EditorGUILayout.ObjectField("Add scene", null, typeof(SceneAsset), false);
        if (added != null)
        {
            prefs.Scenes.Add(AssetDatabase.GetAssetPath(added));
        }

        EditorGUILayout.Space();
        EditorGUILayout.LabelField("Include", EditorStyles.boldLabel);
        prefs.Canvases = EditorGUILayout.Toggle(new GUIContent("uGUI canvases", "Canvas hierarchies (Image, TextMeshPro, layout groups)."), prefs.Canvases);
        prefs.UIToolkit = EditorGUILayout.Toggle(new GUIContent("UI Toolkit documents", "UIDocuments (UXML + USS), laid out in a throwaway panel. Flexbox becomes auto layout where Flexma reproduces it exactly. Needs Unity 2022.3 or newer."), prefs.UIToolkit);

        EditorGUILayout.Space();
        prefs.MaxImageSize = EditorGUILayout.IntPopup("Max image size", prefs.MaxImageSize, new[] { "1024", "2048", "4096" }, new[] { 1024, 2048, 4096 });
        prefs.AlphaMasks = EditorGUILayout.Toggle(new GUIContent("Alpha masks", "Clip a Mask's children by its sprite's shape (rounded screens, round minimaps) instead of its rectangle."), prefs.AlphaMasks);
        prefs.TextOverflow = EditorGUILayout.Toggle(new GUIContent("Text wrap & overflow", "Keep TMP's word wrapping and overflow mode (no-wrap, masking, ellipsis, truncate)."), prefs.TextOverflow);
        prefs.Components = EditorGUILayout.Toggle(new GUIContent("Components", "Prefabs and UXML templates used more than once become components on a Components page, and their uses become instances that follow the master. Switching this rebuilds those layers in Flexma on the next upload."), prefs.Components);
        EditorGUILayout.LabelField("Export folder", FlexmaUIExporter.DefaultOutputFolder, EditorStyles.miniLabel);

        if (EditorGUI.EndChangeCheck())
        {
            prefs.Save();
        }

        EditorGUILayout.Space();
        EditorGUILayout.BeginHorizontal();
        GUI.enabled = prefs.ExportsSomething;
        if (GUILayout.Button("Export only"))
        {
            RunExport(false);
        }
        GUI.enabled = prefs.CanUpload && prefs.ExportsSomething;
        if (GUILayout.Button("Export and upload"))
        {
            RunExport(true);
        }
        bool hasExport = File.Exists(Path.Combine(FlexmaUIExporter.DefaultOutputFolder, FlexmaUIExporter.PagesFile));
        GUI.enabled = prefs.CanUpload && hasExport;
        if (GUILayout.Button("Upload last export"))
        {
            RunUpload();
        }
        GUI.enabled = true;
        EditorGUILayout.EndHorizontal();
        if (GUILayout.Button("Open export folder"))
        {
            Directory.CreateDirectory(FlexmaUIExporter.DefaultOutputFolder);
            EditorUtility.RevealInFinder(FlexmaUIExporter.DefaultOutputFolder);
        }

        if (!string.IsNullOrEmpty(status))
        {
            EditorGUILayout.Space();
            EditorGUILayout.HelpBox(status, MessageType.Info);
        }
        EditorGUILayout.EndScrollView();
    }

    void RunExport(bool upload)
    {
        try
        {
            var result = FlexmaUIExporter.Export(prefs.ExportOptions(), (s, t) => EditorUtility.DisplayCancelableProgressBar("Flexma UI export", s, t));
            status = "Exported: " + result;
            Debug.Log("[Flexma] Exported: " + result + "\n" + result.Folder);
        }
        catch (Exception e)
        {
            status = "Export failed: " + e.Message;
            Debug.LogException(e);
            return;
        }
        finally
        {
            EditorUtility.ClearProgressBar();
        }
        if (upload)
        {
            RunUpload();
        }
    }

    void RunUpload()
    {
        try
        {
            var result = FlexmaUploader.Upload(FlexmaUIExporter.DefaultOutputFolder, prefs.UploadSettings(), (s, t) => EditorUtility.DisplayCancelableProgressBar("Flexma upload", s, t));
            status = "Uploaded: " + result + "\n" + result.FileUrl;
            Debug.Log("[Flexma] Uploaded: " + result + "\n" + result.FileUrl);
        }
        catch (Exception e)
        {
            status = "Upload failed: " + e.Message;
            Debug.LogException(e);
        }
        finally
        {
            EditorUtility.ClearProgressBar();
        }
    }

    /// <summary>Chooses what the next exports include (saved for this project, like the window's Include toggles).</summary>
    public static string Include(bool canvases, bool uiToolkit)
    {
        var p = Prefs.Load();
        p.Canvases = canvases;
        p.UIToolkit = uiToolkit;
        p.Save();
        return "Flexma exports include: " + (canvases ? "uGUI canvases" : "no canvases") + ", " + (uiToolkit ? "UI Toolkit documents" : "no UI Toolkit documents");
    }

    /// <summary>Exports (and uploads) with the window's saved settings, without dialogs. Returns a summary; never prints the token.</summary>
    public static string ExportAndUpload(bool upload)
    {
        var p = Prefs.Load();
        var exported = FlexmaUIExporter.Export(p.ExportOptions());
        string summary = "export: " + exported;
        if (!upload)
        {
            return summary;
        }
        if (!p.CanUpload)
        {
            return summary + "\nupload skipped: set the file link and API token in Tools > Flexma > UI Exporter first";
        }
        var uploaded = FlexmaUploader.Upload(FlexmaUIExporter.DefaultOutputFolder, p.UploadSettings());
        return summary + "\nupload: " + uploaded + "\n" + uploaded.FileUrl;
    }

    /// <summary>Entry point for -executeMethod (see the class summary).</summary>
    public static void Batch()
    {
        var args = Environment.GetCommandLineArgs();
        try
        {
            var p = Prefs.Load();
            string fileArg = Arg(args, "-flexmaFile");
            if (!string.IsNullOrEmpty(fileArg))
            {
                p.File = fileArg;
            }
            string scenesArg = Arg(args, "-flexmaScenes");
            if (!string.IsNullOrEmpty(scenesArg))
            {
                p.Scenes = scenesArg.Split(',').Select(s => s.Trim()).Where(s => s.Length > 0).ToList();
            }
            if (args.Contains("-flexmaUIToolkit"))
            {
                p.UIToolkit = true;
            }
            if (args.Contains("-flexmaNoCanvases"))
            {
                p.Canvases = false;
            }
            string envToken = Environment.GetEnvironmentVariable("FLEXMA_TOKEN");
            if (!string.IsNullOrEmpty(envToken))
            {
                p.Token = envToken;
            }
            var exported = FlexmaUIExporter.Export(p.ExportOptions());
            Debug.Log("[Flexma] Exported: " + exported);
            if (!args.Contains("-flexmaExportOnly"))
            {
                if (!p.CanUpload)
                {
                    throw new FlexmaException("Upload needs -flexmaFile <link> and the FLEXMA_TOKEN environment variable");
                }
                var uploaded = FlexmaUploader.Upload(FlexmaUIExporter.DefaultOutputFolder, p.UploadSettings());
                Debug.Log("[Flexma] Uploaded: " + uploaded + "\n" + uploaded.FileUrl);
            }
        }
        catch (Exception e)
        {
            Debug.LogException(e);
            if (Application.isBatchMode)
            {
                EditorApplication.Exit(1);
            }
        }
    }

    static string Arg(string[] args, string name)
    {
        int i = Array.IndexOf(args, name);
        if ((i < 0) || (i + 1 >= args.Length))
        {
            return null;
        }
        return args[i + 1];
    }
}
