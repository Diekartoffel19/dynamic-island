using System;
using System.Diagnostics;
using System.Linq;
using System.Net.NetworkInformation;
using System.Text;
using System.Windows.Forms;
using System.Windows.Threading;
using DynamicIsland.Native;

namespace DynamicIsland.Services
{
    public class SystemMonitorService : IDisposable
    {
        private readonly DispatcherTimer _fastTimer;
        private readonly DispatcherTimer _telemetryTimer;

        private ulong _prevIdleTime;
        private ulong _prevKernelTime;
        private ulong _prevUserTime;

        private int _lastVolume = -1;
        private bool _lastMuted;
        private bool _lastCapsLock;
        private bool _lastNumLock;
        private PowerLineStatus _lastPowerLine;
        private int _lastBatteryWarnBucket = 100;
        private bool _lastNetOnline;
        private bool _initialized;

        public SystemTelemetrySnapshot LatestTelemetry { get; private set; }
        public event Action<IslandEvent> StatusAlertTriggered;
        public event Action<SystemTelemetrySnapshot> TelemetryUpdated;

        public SystemMonitorService()
        {
            LatestTelemetry = new SystemTelemetrySnapshot
            {
                CpuPercent = 8,
                RamPercent = 40,
                BatteryPercent = 100,
                IsNetworkOnline = true,
                NetworkName = "Connected",
                VolumePercent = 50
            };

            _fastTimer = new DispatcherTimer(DispatcherPriority.Background)
            {
                Interval = TimeSpan.FromMilliseconds(250)
            };
            _fastTimer.Tick += OnFastTick;

            _telemetryTimer = new DispatcherTimer(DispatcherPriority.Background)
            {
                Interval = TimeSpan.FromMilliseconds(1200)
            };
            _telemetryTimer.Tick += OnTelemetryTick;
        }

        public void Start()
        {
            PrimeBaselineState();
            CoreAudioInterop.RegisterVolumeCallback(OnVolumeNotification);
            _fastTimer.Start();
            _telemetryTimer.Start();
            OnTelemetryTick(this, EventArgs.Empty);
        }

        private void PrimeBaselineState()
        {
            int vol;
            bool muted;
            if (CoreAudioInterop.TryGetVolume(out vol, out muted))
            {
                _lastVolume = vol;
                _lastMuted = muted;
                LatestTelemetry.VolumePercent = vol;
                LatestTelemetry.IsMuted = muted;
            }

            _lastCapsLock = Win32Interop.IsKeyToggled(Win32Interop.VK_CAPITAL);
            _lastNumLock = Win32Interop.IsKeyToggled(Win32Interop.VK_NUMLOCK);

            var ps = SystemInformation.PowerStatus;
            _lastPowerLine = ps.PowerLineStatus;
            _lastNetOnline = NetworkInterface.GetIsNetworkAvailable();

            Win32Interop.FILETIME idle, kernel, user;
            if (Win32Interop.GetSystemTimes(out idle, out kernel, out user))
            {
                _prevIdleTime = idle.ToUInt64();
                _prevKernelTime = kernel.ToUInt64();
                _prevUserTime = user.ToUInt64();
            }

            _initialized = true;
        }

        private void OnVolumeNotification(int vol, bool muted)
        {
            if (!_initialized) return;

            LatestTelemetry.VolumePercent = vol;
            LatestTelemetry.IsMuted = muted;

            if (muted != _lastMuted)
            {
                _lastMuted = muted;
                _lastVolume = vol;
                TriggerAlert(new IslandEvent(
                    muted ? "Audio Stummgeschaltet" : "Audio Aktiv",
                    muted ? "Master-Lautstärke ist stumm" : string.Format("Lautstärke auf {0}%", vol),
                    IslandEventType.Audio,
                    muted ? 0 : vol,
                    muted ? "MUTE" : vol + "%",
                    2000));
            }
            else if (_lastVolume >= 0 && Math.Abs(vol - _lastVolume) >= 1)
            {
                _lastVolume = vol;
                TriggerAlert(new IslandEvent(
                    "Lautstärke",
                    string.Format("Master-Pegel {0}%", vol),
                    IslandEventType.Audio,
                    vol,
                    vol + "%",
                    1800));
            }

            var handler = TelemetryUpdated;
            if (handler != null) handler(LatestTelemetry);
        }

        private void OnFastTick(object sender, EventArgs e)
        {
            if (!_initialized) return;

            // Check CapsLock / NumLock
            bool caps = Win32Interop.IsKeyToggled(Win32Interop.VK_CAPITAL);
            if (caps != _lastCapsLock)
            {
                _lastCapsLock = caps;
                TriggerAlert(new IslandEvent(
                    "Feststelltaste (CapsLock)",
                    caps ? "GROSSSCHREIBUNG AKTIV" : "Normaler Modus",
                    IslandEventType.KeyLock,
                    null,
                    caps ? "ON" : "OFF",
                    1700));
            }

            bool num = Win32Interop.IsKeyToggled(Win32Interop.VK_NUMLOCK);
            if (num != _lastNumLock)
            {
                _lastNumLock = num;
                TriggerAlert(new IslandEvent(
                    "NumLock",
                    num ? "Ziffernblock aktiviert" : "Ziffernblock deaktiviert",
                    IslandEventType.KeyLock,
                    null,
                    num ? "NUM" : "OFF",
                    1600));
            }
        }

        private void OnTelemetryTick(object sender, EventArgs e)
        {
            // CPU Usage via Win32 GetSystemTimes
            Win32Interop.FILETIME idleFt, kernelFt, userFt;
            if (Win32Interop.GetSystemTimes(out idleFt, out kernelFt, out userFt))
            {
                ulong idle = idleFt.ToUInt64();
                ulong kernel = kernelFt.ToUInt64();
                ulong user = userFt.ToUInt64();

                ulong idleDiff = idle - _prevIdleTime;
                ulong kernelDiff = kernel - _prevKernelTime;
                ulong userDiff = user - _prevUserTime;
                ulong total = kernelDiff + userDiff;

                if (total > 0)
                {
                    int cpu = (int)Math.Round((double)(total - idleDiff) * 100.0 / total);
                    LatestTelemetry.CpuPercent = Math.Max(0, Math.Min(100, cpu));
                }

                _prevIdleTime = idle;
                _prevKernelTime = kernel;
                _prevUserTime = user;
            }

            // RAM Usage via GlobalMemoryStatusEx
            var mem = new Win32Interop.MEMORYSTATUSEX();
            if (Win32Interop.GlobalMemoryStatusEx(mem))
            {
                LatestTelemetry.RamPercent = (int)mem.dwMemoryLoad;
                LatestTelemetry.RamTotalGb = Math.Round(mem.ullTotalPhys / (1024.0 * 1024.0 * 1024.0), 1);
                LatestTelemetry.RamUsedGb = Math.Round((mem.ullTotalPhys - mem.ullAvailPhys) / (1024.0 * 1024.0 * 1024.0), 1);
            }

            // Battery & Power Line
            var ps = SystemInformation.PowerStatus;
            bool hasBattery = (ps.BatteryChargeStatus & BatteryChargeStatus.NoSystemBattery) == 0;
            LatestTelemetry.HasBattery = hasBattery;
            int battPct = hasBattery ? (int)Math.Round(ps.BatteryLifePercent * 100.0) : 100;
            LatestTelemetry.BatteryPercent = Math.Max(0, Math.Min(100, battPct));
            LatestTelemetry.IsCharging = ps.PowerLineStatus == PowerLineStatus.Online;

            if (_initialized && ps.PowerLineStatus != _lastPowerLine)
            {
                _lastPowerLine = ps.PowerLineStatus;
                bool plugged = ps.PowerLineStatus == PowerLineStatus.Online;
                TriggerAlert(new IslandEvent(
                    plugged ? "Netzteil Angeschlossen" : "Akkubetrieb",
                    string.Format("Ladestand bei {0}%", LatestTelemetry.BatteryPercent),
                    IslandEventType.Power,
                    LatestTelemetry.BatteryPercent,
                    LatestTelemetry.BatteryPercent + "%",
                    2600));
            }

            if (hasBattery && !LatestTelemetry.IsCharging && LatestTelemetry.BatteryPercent <= 20 && _lastBatteryWarnBucket > 20)
            {
                _lastBatteryWarnBucket = 20;
                TriggerAlert(new IslandEvent(
                    "Akku Schwach",
                    string.Format("Nur noch {0}% verbleibend", LatestTelemetry.BatteryPercent),
                    IslandEventType.Warning,
                    LatestTelemetry.BatteryPercent,
                    "LOW",
                    3200));
            }
            else if (LatestTelemetry.BatteryPercent > 25)
            {
                _lastBatteryWarnBucket = 100;
            }

            // Network state
            bool netOnline = NetworkInterface.GetIsNetworkAvailable();
            LatestTelemetry.IsNetworkOnline = netOnline;

            if (_initialized && netOnline != _lastNetOnline)
            {
                _lastNetOnline = netOnline;
                if (netOnline)
                {
                    try
                    {
                        var activeNic = NetworkInterface.GetAllNetworkInterfaces()
                            .FirstOrDefault(n => n.OperationalStatus == OperationalStatus.Up &&
                                                 n.NetworkInterfaceType != NetworkInterfaceType.Loopback);
                        LatestTelemetry.NetworkName = activeNic != null ? activeNic.Name : "Online";
                    }
                    catch
                    {
                        LatestTelemetry.NetworkName = "Online";
                    }
                }
                else
                {
                    LatestTelemetry.NetworkName = "Offline";
                }

                TriggerAlert(new IslandEvent(
                    netOnline ? "Netzwerk Verbunden" : "Netzwerk Getrennt",
                    netOnline ? "Verbindung aktiv (" + LatestTelemetry.NetworkName + ")" : "Keine aktive Verbindung",
                    netOnline ? IslandEventType.Network : IslandEventType.Warning,
                    null,
                    netOnline ? "NET" : "OFF",
                    2800));
            }

            var handler = TelemetryUpdated;
            if (handler != null) handler(LatestTelemetry);
        }

        public void NotifyClipboardChanged(string previewText)
        {
            string clean = (previewText ?? "Inhalt kopiert").Replace("\r", " ").Replace("\n", " ").Trim();
            if (clean.Length > 38)
            {
                clean = clean.Substring(0, 38) + "...";
            }
            TriggerAlert(new IslandEvent(
                "In Zwischenablage kopiert",
                string.IsNullOrEmpty(clean) ? "Element gespeichert" : clean,
                IslandEventType.Clipboard,
                null,
                "COPY",
                2200));
        }

        public void TriggerAlert(IslandEvent ev)
        {
            var handler = StatusAlertTriggered;
            if (handler != null) handler(ev);
        }

        public void Dispose()
        {
            CoreAudioInterop.UnregisterVolumeCallback();
            _fastTimer.Stop();
            _telemetryTimer.Stop();
        }
    }
}
