using System;
using System.Linq;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Text.RegularExpressions;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

/// <summary>
/// Minimal client for the Flexma MCP endpoint: stateless JSON-RPC tools/call over HTTPS with a personal API token
/// (Flexma sidebar > AI Agents > Connected agents &amp; API tokens), plus the short-lived upload links that
/// upload_image / upload_font / upload_nodes hand out.
/// </summary>
public sealed class FlexmaClient : IDisposable
{
    static readonly Regex UploadLink = new Regex(@"https?://\S+/api/(node|image|font)-uploads/[A-Za-z0-9]+");

    readonly HttpClient http;
    readonly string endpoint;
    readonly string token;
    int nextId = 1;

    public FlexmaClient(string serverUrl, string token)
    {
        endpoint = serverUrl.TrimEnd('/') + "/mcp";
        this.token = token;
        http = new HttpClient { Timeout = TimeSpan.FromMinutes(5) };
    }

    public void Dispose()
    {
        http.Dispose();
    }

    /// <summary>Calls a Flexma MCP tool and returns its text output.</summary>
    public string CallTool(string name, JObject arguments)
    {
        var request = new JObject
        {
            ["jsonrpc"] = "2.0",
            ["id"] = nextId++,
            ["method"] = "tools/call",
            ["params"] = new JObject { ["name"] = name, ["arguments"] = arguments }
        };
        using (var message = new HttpRequestMessage(HttpMethod.Post, endpoint))
        {
            message.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token);
            message.Headers.Accept.ParseAdd("application/json");
            message.Headers.Accept.ParseAdd("text/event-stream");
            message.Content = new StringContent(request.ToString(Formatting.None), Encoding.UTF8, "application/json");
            var response = http.SendAsync(message).GetAwaiter().GetResult();
            string body = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
            if (!response.IsSuccessStatusCode)
            {
                throw new FlexmaException(name + " failed (" + (int)response.StatusCode + "): " + Shorten(body));
            }
            var json = ParseRpc(body);
            if (json["error"] != null)
            {
                throw new FlexmaException(name + ": " + (string)json["error"]["message"]);
            }
            var result = json["result"];
            string text = string.Join("\n", result["content"].Where(c => (string)c["type"] == "text").Select(c => (string)c["text"]));
            if ((bool?)result["isError"] == true)
            {
                throw new FlexmaException(name + ": " + text);
            }
            return text;
        }
    }

    /// <summary>Asks an upload tool for a link (the tool prints a curl command containing it).</summary>
    public string RequestUploadLink(string tool, JObject arguments)
    {
        string text = CallTool(tool, arguments);
        var m = UploadLink.Match(text);
        if (!m.Success)
        {
            throw new FlexmaException(tool + " did not return an upload link: " + Shorten(text));
        }
        return m.Value;
    }

    /// <summary>POSTs one file (or a JSON body) to an upload link and returns the JSON reply.</summary>
    public JObject PostToLink(string link, byte[] data, string contentType)
    {
        using (var content = new ByteArrayContent(data))
        {
            content.Headers.ContentType = MediaTypeHeaderValue.Parse(contentType);
            var response = http.PostAsync(link, content).GetAwaiter().GetResult();
            string body = response.Content.ReadAsStringAsync().GetAwaiter().GetResult();
            JObject json = null;
            try
            {
                json = JObject.Parse(body);
            }
            catch (JsonException)
            {
            }
            if (!response.IsSuccessStatusCode)
            {
                string error = Shorten(body);
                if ((json != null) && (json["error"] != null))
                {
                    error = (string)json["error"];
                }
                throw new FlexmaException("Upload failed (" + (int)response.StatusCode + "): " + error);
            }
            if (json == null)
            {
                throw new FlexmaException("Upload returned an unexpected reply: " + Shorten(body));
            }
            return json;
        }
    }

    /// <summary>Plain JSON, or the last data: line of an SSE stream.</summary>
    static JObject ParseRpc(string body)
    {
        string trimmed = body.TrimStart();
        if (trimmed.StartsWith("{"))
        {
            return JObject.Parse(trimmed);
        }
        string last = null;
        foreach (var line in body.Split('\n'))
        {
            if (line.StartsWith("data:"))
            {
                last = line.Substring(5).Trim();
            }
        }
        if (last == null)
        {
            throw new FlexmaException("Unexpected reply from Flexma: " + Shorten(body));
        }
        return JObject.Parse(last);
    }

    static string Shorten(string s)
    {
        if (s == null)
        {
            return "";
        }
        if (s.Length > 300)
        {
            return s.Substring(0, 300) + "…";
        }
        return s;
    }
}

public sealed class FlexmaException : Exception
{
    public FlexmaException(string message) : base(message)
    {
    }
}
