using System;
using System.Linq;
using UnityEditor;
using UnityEngine;

namespace Flexma.Import
{
    /// <summary>
    /// Tools > Flexma > Import from Flexma. The token is kept in EditorPrefs on this machine (shared with the
    /// Unity UI exporter); the file link and frames per project.
    /// </summary>
    public class FlexmaImportWindow : EditorWindow
    {
        const string TokenKey = "FlexmaExporter.Token";
        static string ProjectKey(string name) => "FlexmaImport." + Application.dataPath + "." + name;

        string link, token, frames;
        float scale = 2;
        bool background, check = true;
        Vector2 scroll;
        string log = "";

        [MenuItem("Tools/Flexma/Import from Flexma")]
        static void Open() => GetWindow<FlexmaImportWindow>("Flexma import");

        void OnEnable()
        {
            token = EditorPrefs.GetString(TokenKey, "");
            link = EditorPrefs.GetString(ProjectKey("File"), EditorPrefs.GetString("FlexmaExporter." + Application.dataPath + ".File", ""));
            frames = EditorPrefs.GetString(ProjectKey("Frames"), "");
            scale = EditorPrefs.GetFloat(ProjectKey("Scale"), 2);
            background = EditorPrefs.GetBool(ProjectKey("Background"), false);
        }

        void Save()
        {
            EditorPrefs.SetString(TokenKey, token ?? "");
            EditorPrefs.SetString(ProjectKey("File"), link ?? "");
            EditorPrefs.SetString(ProjectKey("Frames"), frames ?? "");
            EditorPrefs.SetFloat(ProjectKey("Scale"), scale);
            EditorPrefs.SetBool(ProjectKey("Background"), background);
        }

        void OnGUI()
        {
            EditorGUILayout.LabelField("Flexma file", EditorStyles.boldLabel);
            link = EditorGUILayout.TextField(new GUIContent("File link", "https://host/file/ID"), link);
            token = EditorGUILayout.PasswordField(new GUIContent("API token", "Flexma > AI Agents > Connected agents & API tokens"), token);
            EditorGUILayout.LabelField(new GUIContent("Frame ids", "One per line or comma separated: screens, or components to use as prefabs"));
            frames = EditorGUILayout.TextArea(frames, GUILayout.MinHeight(48));
            scale = EditorGUILayout.Slider(new GUIContent("Image scale", "Image pixels per design pixel"), scale, 1, 4);
            background = EditorGUILayout.Toggle(new GUIContent("Frame backgrounds", "Also import each frame's own fill (often a mockup of the game behind the UI)"), background);
            check = EditorGUILayout.Toggle(new GUIContent("Compare with Flexma", "Render each prefab and diff it against Flexma's render"), check);
            using (new EditorGUI.DisabledScope(string.IsNullOrWhiteSpace(link) || string.IsNullOrWhiteSpace(token) || string.IsNullOrWhiteSpace(frames)))
            {
                if (GUILayout.Button("Import"))
                {
                    Save();
                    try
                    {
                        log = Run(link, frames, check, scale, background, token);
                    }
                    catch (Exception e)
                    {
                        log = "Import failed: " + e.Message;
                        Debug.LogException(e);
                    }
                }
            }
            scroll = EditorGUILayout.BeginScrollView(scroll);
            EditorGUILayout.TextArea(log, EditorStyles.wordWrappedLabel);
            EditorGUILayout.EndScrollView();
        }

        /// <summary>
        /// Imports `frames` (ids, comma or newline separated) from the file at `link`, and compares the prefabs with
        /// Flexma when `check` is set. Uses the saved token when none is given. For agents: call it from an
        /// execute-code tool and read the returned summary.
        /// </summary>
        public static string Run(string link, string frames, bool check = true, float scale = 2, bool background = false, string token = null)
        {
            var settings = new FlexmaImporter.Settings
            {
                FileLink = link.Trim(),
                Token = string.IsNullOrEmpty(token) ? EditorPrefs.GetString(TokenKey, "") : token,
                Frames = frames.Split(new[] { ',', '\n', ' ' }, StringSplitOptions.RemoveEmptyEntries).Select(f => f.Trim()).ToArray(),
                Scale = scale,
                Background = background,
            };
            var result = FlexmaImporter.Import(settings);
            string summary = result.ToString();
            if (check)
            {
                foreach (var r in FlexmaImportCheck.Check(result))
                {
                    summary += "\n" + r;
                }
            }
            // Also on disk, for agents whose call timed out before the import ended.
            System.IO.File.WriteAllText(System.IO.Path.Combine(result.ExportDir, "report.txt"), summary);
            Debug.Log("[Flexma import] " + summary);
            return summary;
        }
    }
}
