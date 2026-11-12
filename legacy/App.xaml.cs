using System;
using System.Drawing;
using System.Threading;
using System.Windows;
using System.Windows.Forms;
using DynamicIsland.Services;
using Application = System.Windows.Application;

namespace DynamicIsland
{
    public partial class App : Application
    {
        private Mutex _singleInstanceMutex;
        private NotifyIcon _trayIcon;
        private MainWindow _mainWindow;

        protected override void OnStartup(StartupEventArgs e)
        {
            base.OnStartup(e);

            bool createdNew;
            _singleInstanceMutex = new Mutex(true, "Local\\WindowsDynamicIslandAppMutex_v1", out createdNew);
            if (!createdNew)
            {
                Shutdown();
                return;
            }

            _mainWindow = new MainWindow();
            _mainWindow.Show();

            InitializeTrayIcon();

            Dispatcher.BeginInvoke(System.Windows.Threading.DispatcherPriority.ApplicationIdle, new Action(() =>
            {
                try
                {
                    Native.Win32Interop.SetProcessWorkingSetSize(
                        System.Diagnostics.Process.GetCurrentProcess().Handle,
                        (IntPtr)(-1),
                        (IntPtr)(-1));
                }
                catch
                {
                }
            }));
        }

        private void InitializeTrayIcon()
        {
            _trayIcon = new NotifyIcon
            {
                Text = "Windows Dynamic Island (.NET)",
                Visible = true,
                Icon = CreateIslandTrayIcon()
            };

            var menu = new ContextMenuStrip();

            var expandItem = new ToolStripMenuItem("Dynamic Island öffnen / erweitern");
            expandItem.Click += (s, a) => _mainWindow.ToggleExpandedFromTray();
            menu.Items.Add(expandItem);

            var centerItem = new ToolStripMenuItem("Oben zentrieren");
            centerItem.Click += (s, a) => _mainWindow.CenterOnTopScreen();
            menu.Items.Add(centerItem);

            var clickThroughItem = new ToolStripMenuItem("Click-Through Modus (Maus durchklicken)");
            clickThroughItem.CheckOnClick = true;
            clickThroughItem.CheckedChanged += (s, a) => _mainWindow.SetClickThrough(clickThroughItem.Checked);
            menu.Items.Add(clickThroughItem);

            menu.Items.Add(new ToolStripSeparator());

            var demoMenu = new ToolStripMenuItem("Test-Statusmeldungen senden");
            demoMenu.DropDownItems.Add("Erfolg (Build abgeschlossen)", null, (s, a) =>
                _mainWindow.EnqueueAlert(new IslandEvent("Build Erfolgreich", "DynamicIsland.exe (0 Fehler, 1.1s)", IslandEventType.Success, 100, "OK")));
            demoMenu.DropDownItems.Add("Warnung (Hohe Auslastung)", null, (s, a) =>
                _mainWindow.EnqueueAlert(new IslandEvent("System-Warnung", "Hintergrundprozess nutzt 89% CPU", IslandEventType.Warning, 89, "WARN")));
            demoMenu.DropDownItems.Add("Audio / Musik-Status", null, (s, a) =>
                _mainWindow.EnqueueAlert(new IslandEvent("Medienwiedergabe", "Synthwave FM – Midnight Drive", IslandEventType.Audio, 72, "LIVE")));
            demoMenu.DropDownItems.Add("Fehler-Alarm", null, (s, a) =>
                _mainWindow.EnqueueAlert(new IslandEvent("Verbindungsfehler", "Server Timeout auf Port 8080", IslandEventType.Error, null, "ERR")));
            menu.Items.Add(demoMenu);

            menu.Items.Add(new ToolStripSeparator());

            var exitItem = new ToolStripMenuItem("Beenden");
            exitItem.Click += (s, a) => ExitApplication();
            menu.Items.Add(exitItem);

            _trayIcon.ContextMenuStrip = menu;
            _trayIcon.DoubleClick += (s, a) => _mainWindow.ToggleExpandedFromTray();
        }

        private static Icon CreateIslandTrayIcon()
        {
            using (var bmp = new Bitmap(32, 32))
            using (var g = Graphics.FromImage(bmp))
            {
                g.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
                g.Clear(Color.Transparent);
                using (var bgBrush = new SolidBrush(Color.FromArgb(255, 0, 0, 0)))
                using (var pen = new Pen(Color.FromArgb(255, 90, 162, 255), 2.5f))
                using (var dotBrush = new SolidBrush(Color.FromArgb(255, 139, 108, 255)))
                {
                    g.FillEllipse(bgBrush, 2, 6, 28, 20);
                    g.DrawEllipse(pen, 2, 6, 28, 20);
                    g.FillEllipse(dotBrush, 8, 13, 6, 6);
                }
                return Icon.FromHandle(bmp.GetHicon());
            }
        }

        public void ExitApplication()
        {
            if (_trayIcon != null)
            {
                _trayIcon.Visible = false;
                _trayIcon.Dispose();
            }
            if (_mainWindow != null)
            {
                _mainWindow.Cleanup();
                _mainWindow.Close();
            }
            Shutdown();
        }
    }
}
