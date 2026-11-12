using System;

namespace DynamicIsland.Services
{
    public enum IslandEventType
    {
        Info,
        Success,
        Warning,
        Error,
        Audio,
        Power,
        Network,
        Clipboard,
        KeyLock,
        Timer,
        Bluetooth,
        Usb
    }

    public class IslandEvent
    {
        public string Title { get; set; }
        public string Message { get; set; }
        public IslandEventType Type { get; set; }
        public int? ProgressValue { get; set; } // 0..100 optional progress bar
        public string BadgeText { get; set; }
        public DateTime Timestamp { get; set; }
        public int DurationMs { get; set; }

        public IslandEvent(string title, string message, IslandEventType type = IslandEventType.Info, int? progress = null, string badge = null, int durationMs = 2600)
        {
            Title = title ?? "Status";
            Message = message ?? "";
            Type = type;
            ProgressValue = progress;
            BadgeText = badge;
            Timestamp = DateTime.Now;
            DurationMs = durationMs;
        }
    }

    public class SystemTelemetrySnapshot
    {
        public int CpuPercent { get; set; }
        public int RamPercent { get; set; }
        public double RamUsedGb { get; set; }
        public double RamTotalGb { get; set; }
        public int BatteryPercent { get; set; }
        public bool IsCharging { get; set; }
        public bool HasBattery { get; set; }
        public bool IsNetworkOnline { get; set; }
        public string NetworkName { get; set; }
        public int VolumePercent { get; set; }
        public bool IsMuted { get; set; }
    }
}
