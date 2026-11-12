using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Threading;

namespace DynamicIsland.Services
{
    /// <summary>
    /// Minimal HTTP file server for the local network. Starts on first use.
    ///
    ///   /{fileToken}/{name}      download one shelf file (supports Range / resume)
    ///   /{hubToken}/             web page: download shelf files, upload files, send text
    ///   /{hubToken}/list         shelf files as JSON
    ///   /{hubToken}/upload?name= POST, body = file bytes, saved to the receive folder
    ///   /{hubToken}/text         POST, body = UTF-8 text
    ///
    /// Nothing is reachable without one of the random tokens.
    /// Events are raised on thread-pool threads.
    /// </summary>
    public class FileShareServer : IDisposable
    {
        private const int MaxHeaderBytes = 16 * 1024;
        private const int MaxTextBytes = 1024 * 1024;

        private readonly int _port;
        private readonly object _lock = new object();
        private readonly Dictionary<string, string> _tokenToPath = new Dictionary<string, string>();
        private readonly Dictionary<string, string> _pathToToken = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        private readonly List<string> _order = new List<string>();
        private readonly string _hubToken = CreateToken(8);
        private readonly string _receiveFolder;
        private TcpListener _listener;
        private volatile bool _running;

        /// <summary>A file arrived from another device. Argument: full path of the saved file.</summary>
        public event Action<string> FileReceived;
        /// <summary>Text arrived from another device.</summary>
        public event Action<string> TextReceived;
        /// <summary>A shelf file was downloaded to its end. Argument: file name.</summary>
        public event Action<string> FileDownloaded;

        public FileShareServer(int port)
        {
            _port = port;
            _receiveFolder = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Downloads", "Island");
        }

        public string ReceiveFolder
        {
            get { return _receiveFolder; }
        }

        /// <summary>Makes the file visible on the hub page (does not start the server).</summary>
        public void Publish(string path)
        {
            lock (_lock)
            {
                GetOrCreateToken(path);
            }
        }

        /// <summary>Registers the file and returns its direct download URL.</summary>
        public string Share(string path)
        {
            EnsureStarted();

            string token;
            lock (_lock)
            {
                token = GetOrCreateToken(path);
            }

            return string.Format("http://{0}:{1}/{2}/{3}",
                GetLocalAddress(), _port, token, Uri.EscapeDataString(Path.GetFileName(path)));
        }

        public void Unshare(string path)
        {
            lock (_lock)
            {
                string token;
                if (_pathToToken.TryGetValue(path, out token))
                {
                    _pathToToken.Remove(path);
                    _tokenToPath.Remove(token);
                    _order.RemoveAll(p => string.Equals(p, path, StringComparison.OrdinalIgnoreCase));
                }
            }
        }

        /// <summary>Starts the server if needed and returns the URL of the hub page.</summary>
        public string OpenHub()
        {
            EnsureStarted();
            return string.Format("http://{0}:{1}/{2}/", GetLocalAddress(), _port, _hubToken);
        }

        // Caller holds _lock.
        private string GetOrCreateToken(string path)
        {
            string token;
            if (!_pathToToken.TryGetValue(path, out token))
            {
                token = CreateToken(16);
                _pathToToken[path] = token;
                _tokenToPath[token] = path;
                _order.Add(path);
            }
            return token;
        }

        private void EnsureStarted()
        {
            lock (_lock)
            {
                if (_running) return;

                _listener = new TcpListener(IPAddress.Any, _port);
                _listener.Start();
                _running = true;

                var thread = new Thread(AcceptLoop);
                thread.IsBackground = true;
                thread.Name = "IslandFileShare";
                thread.Start();
            }
        }

        private void AcceptLoop()
        {
            while (_running)
            {
                try
                {
                    TcpClient client = _listener.AcceptTcpClient();
                    ThreadPool.QueueUserWorkItem(HandleClient, client);
                }
                catch
                {
                    if (!_running) return;
                }
            }
        }

        // =====================================================================
        //  Request handling
        // =====================================================================

        private sealed class Request
        {
            public string Method;
            public string Path;
            public string Query;
            public readonly Dictionary<string, string> Headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            public byte[] Leftover;      // body bytes that arrived together with the header
            public int LeftoverOffset;
            public int LeftoverCount;
        }

        private void HandleClient(object state)
        {
            var client = (TcpClient)state;
            try
            {
                client.ReceiveTimeout = 5000;
                client.SendTimeout = 30000;
                using (NetworkStream stream = client.GetStream())
                {
                    Request req = ReadRequest(stream);
                    if (req == null)
                    {
                        WriteStatus(stream, "400 Bad Request");
                        return;
                    }
                    Route(client, stream, req);
                }
            }
            catch
            {
            }
            finally
            {
                try { client.Close(); } catch { }
            }
        }

        private void Route(TcpClient client, NetworkStream stream, Request req)
        {
            string[] segments = req.Path.Trim('/').Split('/');
            bool isGet = req.Method == "GET";
            bool isHead = req.Method == "HEAD";

            if (segments[0] == _hubToken)
            {
                string sub = segments.Length > 1 ? segments[1] : "";

                if (sub == "" && (isGet || isHead))
                {
                    if (!req.Path.EndsWith("/", StringComparison.Ordinal))
                    {
                        // Relative URLs on the page need the trailing slash.
                        WriteResponse(stream, "301 Moved Permanently", null, null, false, "Location: /" + _hubToken + "/\r\n");
                        return;
                    }
                    WriteResponse(stream, "200 OK", "text/html; charset=utf-8", Encoding.UTF8.GetBytes(SharePage.Html), isHead, null);
                }
                else if (sub == "list" && isGet)
                {
                    WriteResponse(stream, "200 OK", "application/json; charset=utf-8", Encoding.UTF8.GetBytes(BuildListJson()), false, null);
                }
                else if (sub == "upload" && req.Method == "POST")
                {
                    HandleUpload(client, stream, req);
                }
                else if (sub == "text" && req.Method == "POST")
                {
                    HandleText(stream, req);
                }
                else
                {
                    WriteStatus(stream, "404 Not Found");
                }
                return;
            }

            if (!isGet && !isHead)
            {
                WriteStatus(stream, "405 Method Not Allowed");
                return;
            }

            string path;
            lock (_lock)
            {
                _tokenToPath.TryGetValue(segments[0], out path);
            }

            if (path == null || !File.Exists(path))
            {
                WriteStatus(stream, "404 Not Found");
                return;
            }

            ServeFile(stream, req, path, isHead);
        }

        private void ServeFile(NetworkStream stream, Request req, string path, bool isHead)
        {
            string name = Path.GetFileName(path);
            bool reachedEnd;

            using (var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete))
            {
                long length = file.Length;
                long start = 0, end = length - 1;
                bool partial = false;

                string range;
                if (req.Headers.TryGetValue("Range", out range))
                {
                    bool unsatisfiable;
                    partial = TryParseRange(range, length, out start, out end, out unsatisfiable);
                    if (unsatisfiable)
                    {
                        WriteResponse(stream, "416 Range Not Satisfiable", null, null, false, "Content-Range: bytes */" + length + "\r\n");
                        return;
                    }
                    if (!partial)
                    {
                        start = 0;
                        end = length - 1;
                    }
                }

                long count = end - start + 1;
                var header = new StringBuilder();
                header.Append(partial ? "HTTP/1.1 206 Partial Content\r\n" : "HTTP/1.1 200 OK\r\n");
                header.Append("Content-Type: application/octet-stream\r\n");
                header.Append("Content-Length: ").Append(count).Append("\r\n");
                header.Append("Accept-Ranges: bytes\r\n");
                if (partial)
                {
                    header.Append("Content-Range: bytes ").Append(start).Append('-').Append(end).Append('/').Append(length).Append("\r\n");
                }
                header.Append("Content-Disposition: attachment; filename=\"").Append(ToAsciiName(name))
                      .Append("\"; filename*=UTF-8''").Append(Uri.EscapeDataString(name)).Append("\r\n");
                header.Append("X-Content-Type-Options: nosniff\r\n");
                header.Append("Connection: close\r\n\r\n");

                byte[] headerBytes = Encoding.ASCII.GetBytes(header.ToString());
                stream.Write(headerBytes, 0, headerBytes.Length);
                if (isHead) return;

                file.Position = start;
                var buffer = new byte[64 * 1024];
                long remaining = count;
                while (remaining > 0)
                {
                    int read = file.Read(buffer, 0, (int)Math.Min(buffer.Length, remaining));
                    if (read <= 0) break;
                    stream.Write(buffer, 0, read);
                    remaining -= read;
                }
                reachedEnd = remaining == 0 && end == length - 1;
            }

            if (reachedEnd) Raise(FileDownloaded, name);
        }

        private void HandleUpload(TcpClient client, NetworkStream stream, Request req)
        {
            long length;
            if (!TryGetContentLength(req, out length))
            {
                WriteStatus(stream, "411 Length Required");
                return;
            }

            string name = SanitizeFileName(GetQueryValue(req.Query, "name"));
            Directory.CreateDirectory(_receiveFolder);

            try
            {
                var drive = new DriveInfo(Path.GetPathRoot(_receiveFolder));
                if (drive.AvailableFreeSpace < length)
                {
                    WriteStatus(stream, "507 Insufficient Storage");
                    return;
                }
            }
            catch
            {
            }

            string temp = Path.Combine(_receiveFolder, "." + CreateToken(6) + ".part");
            try
            {
                SendContinueIfExpected(stream, req);
                client.ReceiveTimeout = 30000;

                bool complete;
                using (var output = new FileStream(temp, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                {
                    complete = CopyBody(stream, req, length, output);
                }
                if (!complete)
                {
                    WriteStatus(stream, "400 Bad Request");
                    return;
                }

                string final;
                lock (_lock)
                {
                    final = UniquePath(_receiveFolder, name);
                    File.Move(temp, final);
                }

                WriteResponse(stream, "200 OK", "application/json; charset=utf-8", Encoding.UTF8.GetBytes("{\"ok\":true}"), false, null);
                Raise(FileReceived, final);
            }
            finally
            {
                try
                {
                    if (File.Exists(temp)) File.Delete(temp);
                }
                catch
                {
                }
            }
        }

        private void HandleText(NetworkStream stream, Request req)
        {
            long length;
            if (!TryGetContentLength(req, out length))
            {
                WriteStatus(stream, "411 Length Required");
                return;
            }
            if (length > MaxTextBytes)
            {
                WriteStatus(stream, "413 Payload Too Large");
                return;
            }

            SendContinueIfExpected(stream, req);
            var body = new MemoryStream();
            if (!CopyBody(stream, req, length, body))
            {
                WriteStatus(stream, "400 Bad Request");
                return;
            }

            string text = Encoding.UTF8.GetString(body.ToArray());
            WriteResponse(stream, "200 OK", "application/json; charset=utf-8", Encoding.UTF8.GetBytes("{\"ok\":true}"), false, null);
            if (text.Length > 0) Raise(TextReceived, text);
        }

        private string BuildListJson()
        {
            var entries = new List<KeyValuePair<string, string>>();
            lock (_lock)
            {
                foreach (string path in _order)
                {
                    entries.Add(new KeyValuePair<string, string>(_pathToToken[path], path));
                }
            }

            var sb = new StringBuilder("[");
            bool first = true;
            foreach (KeyValuePair<string, string> entry in entries)
            {
                long size;
                try
                {
                    var info = new FileInfo(entry.Value);
                    if (!info.Exists) continue;
                    size = info.Length;
                }
                catch
                {
                    continue;
                }

                string name = Path.GetFileName(entry.Value);
                if (!first) sb.Append(',');
                first = false;
                sb.Append("{\"name\":\"").Append(EscapeJson(name))
                  .Append("\",\"size\":").Append(size.ToString(CultureInfo.InvariantCulture))
                  .Append(",\"url\":\"/").Append(entry.Key).Append('/').Append(Uri.EscapeDataString(name)).Append("\"}");
            }
            return sb.Append(']').ToString();
        }

        // =====================================================================
        //  HTTP plumbing
        // =====================================================================

        private static Request ReadRequest(NetworkStream stream)
        {
            var buffer = new byte[MaxHeaderBytes];
            int count = 0;
            while (count < buffer.Length)
            {
                int read = stream.Read(buffer, count, buffer.Length - count);
                if (read <= 0) return null;

                int scanFrom = Math.Max(0, count - 3);
                count += read;

                for (int i = scanFrom; i + 3 < count; i++)
                {
                    if (buffer[i] == '\r' && buffer[i + 1] == '\n' && buffer[i + 2] == '\r' && buffer[i + 3] == '\n')
                    {
                        return ParseRequest(buffer, i, count);
                    }
                }
            }
            return null;
        }

        private static Request ParseRequest(byte[] buffer, int headerEnd, int count)
        {
            string[] lines = Encoding.ASCII.GetString(buffer, 0, headerEnd).Split(new[] { "\r\n" }, StringSplitOptions.None);
            string[] parts = lines[0].Split(' ');
            if (parts.Length < 2 || !parts[1].StartsWith("/", StringComparison.Ordinal)) return null;

            var req = new Request();
            req.Method = parts[0];

            int q = parts[1].IndexOf('?');
            req.Path = q < 0 ? parts[1] : parts[1].Substring(0, q);
            req.Query = q < 0 ? "" : parts[1].Substring(q + 1);

            for (int i = 1; i < lines.Length; i++)
            {
                int colon = lines[i].IndexOf(':');
                if (colon > 0) req.Headers[lines[i].Substring(0, colon).Trim()] = lines[i].Substring(colon + 1).Trim();
            }

            req.Leftover = buffer;
            req.LeftoverOffset = headerEnd + 4;
            req.LeftoverCount = count - req.LeftoverOffset;
            return req;
        }

        private static bool TryGetContentLength(Request req, out long length)
        {
            length = 0;
            string value;
            return req.Headers.TryGetValue("Content-Length", out value)
                && long.TryParse(value, NumberStyles.None, CultureInfo.InvariantCulture, out length);
        }

        private static void SendContinueIfExpected(NetworkStream stream, Request req)
        {
            string expect;
            if (req.Headers.TryGetValue("Expect", out expect) && expect.IndexOf("100-continue", StringComparison.OrdinalIgnoreCase) >= 0)
            {
                byte[] bytes = Encoding.ASCII.GetBytes("HTTP/1.1 100 Continue\r\n\r\n");
                stream.Write(bytes, 0, bytes.Length);
            }
        }

        /// <summary>Copies exactly <paramref name="length"/> body bytes; false if the peer hung up early.</summary>
        private static bool CopyBody(NetworkStream stream, Request req, long length, Stream output)
        {
            long remaining = length;

            int fromHeader = (int)Math.Min(req.LeftoverCount, remaining);
            if (fromHeader > 0)
            {
                output.Write(req.Leftover, req.LeftoverOffset, fromHeader);
                remaining -= fromHeader;
            }

            var buffer = new byte[64 * 1024];
            while (remaining > 0)
            {
                int read = stream.Read(buffer, 0, (int)Math.Min(buffer.Length, remaining));
                if (read <= 0) return false;
                output.Write(buffer, 0, read);
                remaining -= read;
            }
            return true;
        }

        /// <summary>
        /// True for a usable single range. A malformed or multi-part header is ignored
        /// (whole file is served); <paramref name="unsatisfiable"/> asks for a 416.
        /// </summary>
        private static bool TryParseRange(string header, long length, out long start, out long end, out bool unsatisfiable)
        {
            start = 0;
            end = length - 1;
            unsatisfiable = false;

            if (!header.StartsWith("bytes=", StringComparison.OrdinalIgnoreCase)) return false;
            string spec = header.Substring(6).Trim();
            int dash = spec.IndexOf('-');
            if (dash < 0 || spec.IndexOf(',') >= 0) return false;

            string a = spec.Substring(0, dash).Trim();
            string b = spec.Substring(dash + 1).Trim();
            long first, last;

            if (a.Length == 0)
            {
                // Suffix range: the last N bytes.
                if (!long.TryParse(b, NumberStyles.None, CultureInfo.InvariantCulture, out last)) return false;
                if (last <= 0 || length == 0)
                {
                    unsatisfiable = true;
                    return false;
                }
                start = Math.Max(0, length - last);
                return true;
            }

            if (!long.TryParse(a, NumberStyles.None, CultureInfo.InvariantCulture, out first)) return false;
            last = length - 1;
            if (b.Length > 0)
            {
                if (!long.TryParse(b, NumberStyles.None, CultureInfo.InvariantCulture, out last)) return false;
                last = Math.Min(last, length - 1);
            }
            if (first >= length || first > last)
            {
                unsatisfiable = true;
                return false;
            }

            start = first;
            end = last;
            return true;
        }

        private static void WriteStatus(NetworkStream stream, string status)
        {
            WriteResponse(stream, status, null, null, false, null);
        }

        private static void WriteResponse(NetworkStream stream, string status, string contentType, byte[] body, bool headOnly, string extraHeaders)
        {
            int length = body == null ? 0 : body.Length;
            var header = new StringBuilder();
            header.Append("HTTP/1.1 ").Append(status).Append("\r\n");
            if (contentType != null) header.Append("Content-Type: ").Append(contentType).Append("\r\n");
            header.Append("Content-Length: ").Append(length).Append("\r\n");
            if (extraHeaders != null) header.Append(extraHeaders);
            header.Append("Cache-Control: no-store\r\n");
            header.Append("X-Content-Type-Options: nosniff\r\n");
            header.Append("Referrer-Policy: no-referrer\r\n");
            header.Append("Connection: close\r\n\r\n");

            byte[] headerBytes = Encoding.ASCII.GetBytes(header.ToString());
            stream.Write(headerBytes, 0, headerBytes.Length);
            if (length > 0 && !headOnly) stream.Write(body, 0, length);
        }

        private static void Raise(Action<string> handler, string argument)
        {
            if (handler == null) return;
            try
            {
                handler(argument);
            }
            catch
            {
            }
        }

        // =====================================================================
        //  Helpers
        // =====================================================================

        private static string GetQueryValue(string query, string key)
        {
            foreach (string pair in query.Split('&'))
            {
                int eq = pair.IndexOf('=');
                if (eq <= 0 || pair.Substring(0, eq) != key) continue;
                try
                {
                    return Uri.UnescapeDataString(pair.Substring(eq + 1).Replace('+', ' '));
                }
                catch
                {
                    return null;
                }
            }
            return null;
        }

        /// <summary>Reduces a name sent by a remote device to a harmless file name (no path parts).</summary>
        private static string SanitizeFileName(string raw)
        {
            string name = raw ?? "";
            int slash = name.LastIndexOfAny(new[] { '/', '\\' });
            if (slash >= 0) name = name.Substring(slash + 1);

            char[] invalid = Path.GetInvalidFileNameChars();
            var sb = new StringBuilder(name.Length);
            foreach (char c in name)
            {
                sb.Append(c < 32 || Array.IndexOf(invalid, c) >= 0 ? '_' : c);
            }
            name = sb.ToString().Trim().TrimEnd('.', ' ');

            if (name.Length > 150)
            {
                string ext = Path.GetExtension(name);
                if (ext.Length > 20) ext = "";
                name = name.Substring(0, 150 - ext.Length) + ext;
            }
            if (name.Length == 0) name = "Datei";

            // Windows device names (CON, NUL, COM1 ...) are not usable as file names.
            string stem = name.Split('.')[0].Trim().ToUpperInvariant();
            bool reserved = stem == "CON" || stem == "PRN" || stem == "AUX" || stem == "NUL" ||
                (stem.Length == 4 && (stem.StartsWith("COM", StringComparison.Ordinal) || stem.StartsWith("LPT", StringComparison.Ordinal)) && char.IsDigit(stem[3]));
            if (reserved) name = "_" + name;

            return name;
        }

        private static string UniquePath(string folder, string name)
        {
            string candidate = Path.Combine(folder, name);
            string stem = Path.GetFileNameWithoutExtension(name);
            string ext = Path.GetExtension(name);
            for (int i = 2; File.Exists(candidate) || Directory.Exists(candidate); i++)
            {
                candidate = Path.Combine(folder, string.Format("{0} ({1}){2}", stem, i, ext));
            }
            return candidate;
        }

        private static string EscapeJson(string s)
        {
            var sb = new StringBuilder(s.Length + 8);
            foreach (char c in s)
            {
                if (c == '"' || c == '\\') sb.Append('\\').Append(c);
                else if (c < 32) sb.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                else sb.Append(c);
            }
            return sb.ToString();
        }

        private static string ToAsciiName(string name)
        {
            var sb = new StringBuilder(name.Length);
            foreach (char c in name)
            {
                sb.Append(c >= 32 && c < 127 && c != '"' && c != '\\' ? c : '_');
            }
            return sb.ToString();
        }

        private static string CreateToken(int byteCount)
        {
            var bytes = new byte[byteCount];
            using (var rng = new RNGCryptoServiceProvider())
            {
                rng.GetBytes(bytes);
            }
            var sb = new StringBuilder(byteCount * 2);
            foreach (byte b in bytes) sb.Append(b.ToString("x2"));
            return sb.ToString();
        }

        private static string GetLocalAddress()
        {
            string fallback = null;
            try
            {
                foreach (NetworkInterface nic in NetworkInterface.GetAllNetworkInterfaces())
                {
                    if (nic.OperationalStatus != OperationalStatus.Up) continue;
                    if (nic.NetworkInterfaceType == NetworkInterfaceType.Loopback ||
                        nic.NetworkInterfaceType == NetworkInterfaceType.Tunnel) continue;

                    IPInterfaceProperties props = nic.GetIPProperties();
                    foreach (UnicastIPAddressInformation ua in props.UnicastAddresses)
                    {
                        if (ua.Address.AddressFamily != AddressFamily.InterNetwork || IPAddress.IsLoopback(ua.Address)) continue;
                        if (props.GatewayAddresses.Count > 0) return ua.Address.ToString();
                        if (fallback == null) fallback = ua.Address.ToString();
                    }
                }
            }
            catch
            {
            }
            return fallback ?? "127.0.0.1";
        }

        public void Dispose()
        {
            _running = false;
            try
            {
                if (_listener != null) _listener.Stop();
            }
            catch
            {
            }
        }
    }
}
