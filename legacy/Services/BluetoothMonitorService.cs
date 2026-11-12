using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

namespace DynamicIsland.Services
{
    /// <summary>
    /// Polls the paired Bluetooth devices and raises an alert when one connects or disconnects.
    /// Uses the classic Bluetooth API (bthprops.cpl), so Bluetooth LE-only devices are not reported.
    /// Events are raised on a background thread.
    /// </summary>
    public class BluetoothMonitorService : IDisposable
    {
        private const int PollIntervalMs = 2000;

        private readonly ManualResetEvent _stopSignal = new ManualResetEvent(false);
        private Thread _thread;
        private Dictionary<ulong, bool> _lastConnected;

        public event Action<IslandEvent> StatusAlertTriggered;

        public void Start()
        {
            if (_thread != null) return;

            _thread = new Thread(PollLoop)
            {
                IsBackground = true,
                Name = "BluetoothMonitor"
            };
            _thread.Start();
        }

        private void PollLoop()
        {
            do
            {
                try
                {
                    PollOnce();
                }
                catch
                {
                    // No Bluetooth stack on this machine, or a transient API failure: try again later.
                }
            }
            while (!_stopSignal.WaitOne(PollIntervalMs));
        }

        private void PollOnce()
        {
            var names = new Dictionary<ulong, string>();
            var classes = new Dictionary<ulong, uint>();
            var connected = new Dictionary<ulong, bool>();

            var search = new BLUETOOTH_DEVICE_SEARCH_PARAMS();
            search.dwSize = (uint)Marshal.SizeOf(typeof(BLUETOOTH_DEVICE_SEARCH_PARAMS));
            search.fReturnAuthenticated = true;
            search.fReturnRemembered = true;
            search.fReturnConnected = true;
            search.fReturnUnknown = false;
            search.fIssueInquiry = false;
            search.cTimeoutMultiplier = 0;
            search.hRadio = IntPtr.Zero;

            var info = new BLUETOOTH_DEVICE_INFO();
            info.dwSize = (uint)Marshal.SizeOf(typeof(BLUETOOTH_DEVICE_INFO));

            IntPtr find = BluetoothFindFirstDevice(ref search, ref info);
            if (find != IntPtr.Zero)
            {
                try
                {
                    do
                    {
                        names[info.Address] = info.szName;
                        classes[info.Address] = info.ulClassofDevice;
                        connected[info.Address] = info.fConnected;

                        info = new BLUETOOTH_DEVICE_INFO();
                        info.dwSize = (uint)Marshal.SizeOf(typeof(BLUETOOTH_DEVICE_INFO));
                    }
                    while (BluetoothFindNextDevice(find, ref info));
                }
                finally
                {
                    BluetoothFindDeviceClose(find);
                }
            }

            // The first pass only records the baseline, so already connected devices do not toast at startup.
            if (_lastConnected != null)
            {
                foreach (var pair in connected)
                {
                    bool was;
                    _lastConnected.TryGetValue(pair.Key, out was);
                    if (pair.Value != was)
                    {
                        Raise(names[pair.Key], classes[pair.Key], pair.Value);
                    }
                }
            }

            _lastConnected = connected;
        }

        private void Raise(string name, uint classOfDevice, bool isConnected)
        {
            string kind = DescribeClass(classOfDevice);
            string title = string.IsNullOrWhiteSpace(name) ? kind : name.Trim();

            var ev = new IslandEvent(
                title,
                isConnected ? kind + " verbunden" : kind + " getrennt",
                isConnected ? IslandEventType.Bluetooth : IslandEventType.Warning,
                null,
                isConnected ? "BT" : "OFF",
                2800);

            var handler = StatusAlertTriggered;
            if (handler != null) handler(ev);
        }

        // Major device class: bits 8..12 of the Bluetooth class of device.
        private static string DescribeClass(uint classOfDevice)
        {
            uint major = (classOfDevice >> 8) & 0x1F;
            uint minor = (classOfDevice >> 2) & 0x3F;

            switch (major)
            {
                case 1:
                    return "Computer";
                case 2:
                    return "Handy";
                case 4:
                    return minor == 5 || minor == 6 ? "Lautsprecher" : "Kopfhörer";
                case 5:
                    if ((minor & 0x30) == 0x10) return "Tastatur";
                    if ((minor & 0x30) == 0x20) return "Maus";
                    if ((minor & 0x0F) == 1 || (minor & 0x0F) == 2) return "Controller";
                    return "Eingabegerät";
                case 6:
                    return "Drucker";
                case 7:
                    return "Wearable";
                default:
                    return "Bluetooth-Gerät";
            }
        }

        public void Dispose()
        {
            _stopSignal.Set();
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct SYSTEMTIME
        {
            public ushort wYear;
            public ushort wMonth;
            public ushort wDayOfWeek;
            public ushort wDay;
            public ushort wHour;
            public ushort wMinute;
            public ushort wSecond;
            public ushort wMilliseconds;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct BLUETOOTH_DEVICE_INFO
        {
            public uint dwSize;
            public ulong Address;
            public uint ulClassofDevice;
            [MarshalAs(UnmanagedType.Bool)] public bool fConnected;
            [MarshalAs(UnmanagedType.Bool)] public bool fRemembered;
            [MarshalAs(UnmanagedType.Bool)] public bool fAuthenticated;
            public SYSTEMTIME stLastSeen;
            public SYSTEMTIME stLastUsed;
            [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 248)] public string szName;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct BLUETOOTH_DEVICE_SEARCH_PARAMS
        {
            public uint dwSize;
            [MarshalAs(UnmanagedType.Bool)] public bool fReturnAuthenticated;
            [MarshalAs(UnmanagedType.Bool)] public bool fReturnRemembered;
            [MarshalAs(UnmanagedType.Bool)] public bool fReturnUnknown;
            [MarshalAs(UnmanagedType.Bool)] public bool fReturnConnected;
            [MarshalAs(UnmanagedType.Bool)] public bool fIssueInquiry;
            public byte cTimeoutMultiplier;
            public IntPtr hRadio;
        }

        [DllImport("bthprops.cpl", SetLastError = true)]
        private static extern IntPtr BluetoothFindFirstDevice(ref BLUETOOTH_DEVICE_SEARCH_PARAMS searchParams, ref BLUETOOTH_DEVICE_INFO deviceInfo);

        [DllImport("bthprops.cpl", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool BluetoothFindNextDevice(IntPtr hFind, ref BLUETOOTH_DEVICE_INFO deviceInfo);

        [DllImport("bthprops.cpl", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool BluetoothFindDeviceClose(IntPtr hFind);
    }
}
