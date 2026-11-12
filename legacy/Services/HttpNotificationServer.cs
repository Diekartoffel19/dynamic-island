using System;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
using System.Web;

namespace DynamicIsland.Services
{
    /// <summary>
    /// Lightweight local HTTP notification server on 127.0.0.1:19845 (no admin privileges required).
    /// Example:
    /// GET http://127.0.0.1:19845/notify?title=Build+OK&msg=Alle+Tests+gruen&type=success&progress=100
    /// </summary>
    public class HttpNotificationServer : IDisposable
    {
        private TcpListener _listener;
        private Thread _thread;
        private volatile bool _running;
        public int Port { get; private set; }

        public event Action<IslandEvent> NotificationReceived;

        public HttpNotificationServer(int port = 19845)
        {
            Port = port;
        }

        public void Start()
        {
            try
            {
                _listener = new TcpListener(IPAddress.Loopback, Port);
                _listener.Start();
                _running = true;
                _thread = new Thread(ListenLoop)
                {
                    IsBackground = true,
                    Name = "IslandHttpNotificationServer"
                };
                _thread.Start();
            }
            catch
            {
                // Port already in use or restricted; ignore gracefully
            }
        }

        private void ListenLoop()
        {
            while (_running)
            {
                try
                {
                    TcpClient client = _listener.AcceptTcpClient();
                    ThreadPool.QueueUserWorkItem(_ => HandleClient(client));
                }
                catch
                {
                    if (!_running) break;
                }
            }
        }

        private void HandleClient(TcpClient client)
        {
            using (client)
            {
                try
                {
                    client.ReceiveTimeout = 2000;
                    client.SendTimeout = 2000;
                    using (NetworkStream stream = client.GetStream())
                    using (var reader = new StreamReader(stream, Encoding.UTF8, false, 1024, true))
                    {
                        string requestLine = reader.ReadLine();
                        if (string.IsNullOrEmpty(requestLine)) return;

                        string[] parts = requestLine.Split(' ');
                        if (parts.Length < 2) return;

                        string rawUrl = parts[1];
                        Uri uri = new Uri("http://127.0.0.1:" + Port + rawUrl);
                        var query = HttpUtility.ParseQueryString(uri.Query);

                        string title = query["title"] ?? "System-Status";
                        string msg = query["msg"] ?? query["message"] ?? "Neue Statusmeldung empfangen";
                        string typeStr = (query["type"] ?? "info").ToLowerInvariant();
                        string badge = query["badge"];
                        int? progress = null;
                        int pVal;
                        if (int.TryParse(query["progress"], out pVal))
                        {
                            progress = Math.Max(0, Math.Min(100, pVal));
                        }

                        IslandEventType evType = IslandEventType.Info;
                        switch (typeStr)
                        {
                            case "success":
                            case "ok":
                                evType = IslandEventType.Success;
                                break;
                            case "warn":
                            case "warning":
                                evType = IslandEventType.Warning;
                                break;
                            case "err":
                            case "error":
                                evType = IslandEventType.Error;
                                break;
                            case "audio":
                            case "music":
                                evType = IslandEventType.Audio;
                                break;
                            case "power":
                            case "battery":
                                evType = IslandEventType.Power;
                                break;
                            case "timer":
                                evType = IslandEventType.Timer;
                                break;
                        }

                        var islandEvent = new IslandEvent(title, msg, evType, progress, badge ?? typeStr.ToUpperInvariant(), 3000);
                        var handler = NotificationReceived;
                        if (handler != null) handler(islandEvent);

                        string json = "{\"ok\":true,\"title\":\"" + EscapeJson(title) + "\",\"message\":\"" + EscapeJson(msg) + "\"}";
                        byte[] bodyBytes = Encoding.UTF8.GetBytes(json);
                        string header = "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: " + bodyBytes.Length + "\r\nConnection: close\r\n\r\n";
                        byte[] headerBytes = Encoding.UTF8.GetBytes(header);
                        stream.Write(headerBytes, 0, headerBytes.Length);
                        stream.Write(bodyBytes, 0, bodyBytes.Length);
                        stream.Flush();
                    }
                }
                catch
                {
                }
            }
        }

        private static string EscapeJson(string s)
        {
            if (string.IsNullOrEmpty(s)) return "";
            return s.Replace("\\", "\\\\").Replace("\"", "\\\"");
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
