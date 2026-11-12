using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Effects;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using DynamicIsland.Native;
using DynamicIsland.Services;

namespace DynamicIsland
{
    public enum IslandVisualState
    {
        Compact,
        Toast,
        Expanded
    }

    public partial class MainWindow : Window
    {
        private IntPtr _hwnd;
        private HwndSource _hwndSource;
        private readonly SystemMonitorService _monitorService;
        private readonly HttpNotificationServer _httpServer;
        private readonly BluetoothMonitorService _bluetoothService;
        private readonly FileShareServer _shareServer;

        private readonly DispatcherTimer _clockTimer;
        private readonly DispatcherTimer _waveTimer;
        private readonly DispatcherTimer _toastDismissTimer;
        private readonly DispatcherTimer _topMostWatchdogTimer;
        private readonly DispatcherTimer _focusCountdownTimer;
        private readonly DispatcherTimer _hoverTimer;
        private readonly DispatcherTimer _leaveTimer;
        private readonly DispatcherTimer _pickTimer;

        private readonly bool _ready;
        private IslandVisualState _currentState = IslandVisualState.Compact;
        private bool _isPinned;
        private bool _isClickThrough;
        private bool _holdOpen;
        private bool _dragOver;
        private bool _suppressVolumeCallback;
        private readonly Random _random = new Random();

        // ---- Shape per state: width, height, corner radius ----
        private const double CompactWidth = 190, CompactHeight = 34, CompactRadius = 17;
        private const double ToastWidth = 344, ToastHeight = 58, ToastRadius = 29;
        private const double ExpandedWidth = 420, ExpandedHeight = 198, ExpandedRadius = 38;

        // ---- Spring (interruptible, keeps velocity between retargets) ----
        private const double SpringStiffness = 260;
        private const double SpringDamping = 24;
        private readonly Stopwatch _springClock = Stopwatch.StartNew();
        private readonly RectangleGeometry _contentClip = new RectangleGeometry();
        private bool _springActive;
        private double _springLastTime;
        private double _w = CompactWidth, _h = CompactHeight, _r = CompactRadius;
        private double _vw, _vh, _vr;
        private double _tw = CompactWidth, _th = CompactHeight, _tr = CompactRadius;

        // ---- Tabs ----
        private FrameworkElement[] _tabPanels;
        private int _tabIndex;
        private const int TabFilesIndex = 2;

        // ---- Focus timer ----
        private int _focusTotalSeconds = 25 * 60;
        private int _focusRemainingSeconds = 25 * 60;
        private bool _focusRunning;

        // ---- Colour picker ----
        private const int LoupeSize = 11;
        private Win32Interop.LowLevelMouseProc _mouseProc;
        private IntPtr _mouseHook = IntPtr.Zero;
        private bool _picking;
        private int _pickResult; // 0 = running, 1 = picked, 2 = cancelled
        private System.Drawing.Bitmap _loupeBitmap;
        private WriteableBitmap _loupeSource;
        private readonly byte[] _loupeBuffer = new byte[LoupeSize * LoupeSize * 4];
        private Color _currentColor = Color.FromRgb(90, 162, 255);
        private Color _colorBeforePick;
        private readonly List<Color> _colorHistory = new List<Color>();

        // ---- File shelf ----
        private readonly List<string> _shelf = new List<string>();
        private Point? _chipDragStart;
        private bool _selfDrag;
        private bool _shareOpen;
        private string _shareUrl;
        private DateTime _ignoreClipboardUntil = DateTime.MinValue;
        private const string ShareDefaultHint = "Ablage laden, Dateien und Text an den PC senden";
        private static readonly string ShelfStatePath = System.IO.Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "DynamicIsland", "shelf.txt");

        private static readonly Color AccentColor = Color.FromRgb(90, 162, 255);
        private static readonly Color GreenColor = Color.FromRgb(48, 209, 88);
        private static readonly Color OrangeColor = Color.FromRgb(255, 159, 10);
        private static readonly Color RedColor = Color.FromRgb(255, 69, 58);
        private static readonly CultureInfo German = new CultureInfo("de-DE");
        private static readonly IEasingFunction EaseOut = CreateEaseOut();
        private static readonly IEasingFunction SharedSineEase = CreateSineEase();

        private static IEasingFunction CreateEaseOut()
        {
            var e = new CubicEase { EasingMode = EasingMode.EaseOut };
            e.Freeze();
            return e;
        }

        private static IEasingFunction CreateSineEase()
        {
            var e = new SineEase { EasingMode = EasingMode.EaseInOut };
            e.Freeze();
            return e;
        }

        public MainWindow()
        {
            InitializeComponent();

            ContentHost.Clip = _contentClip;
            ApplyShape();
            _tabPanels = new FrameworkElement[] { TabHome, TabColor, TabFiles, TabFocus };

            _monitorService = new SystemMonitorService();
            _monitorService.StatusAlertTriggered += OnStatusAlert;
            _monitorService.TelemetryUpdated += OnTelemetryUpdated;

            _bluetoothService = new BluetoothMonitorService();
            _bluetoothService.StatusAlertTriggered += OnStatusAlert;

            _httpServer = new HttpNotificationServer(19845);
            _httpServer.NotificationReceived += ev =>
            {
                Dispatcher.BeginInvoke(new Action(() => EnqueueAlert(ev)));
            };

            _shareServer = new FileShareServer(19846);
            _shareServer.FileReceived += path => Dispatcher.BeginInvoke(new Action(() => OnFileReceived(path)));
            _shareServer.TextReceived += text => Dispatcher.BeginInvoke(new Action(() => OnTextReceived(text)));
            _shareServer.FileDownloaded += name => Dispatcher.BeginInvoke(new Action(() => OnFileDownloaded(name)));

            _clockTimer = new DispatcherTimer();
            _clockTimer.Tick += OnClockTick;

            _waveTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(600) };
            _waveTimer.Tick += OnWaveTick;

            _toastDismissTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(2600) };
            _toastDismissTimer.Tick += (s, e) =>
            {
                _toastDismissTimer.Stop();
                if (_currentState == IslandVisualState.Toast)
                {
                    TransitionToState(IslandVisualState.Compact);
                }
            };

            _topMostWatchdogTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1.5) };
            _topMostWatchdogTimer.Tick += (s, e) => Win32Interop.EnsureTopMost(_hwnd);

            _focusCountdownTimer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
            _focusCountdownTimer.Tick += OnFocusCountdownTick;

            // Small intent delay so brushing past the island does not open it.
            _hoverTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(90) };
            _hoverTimer.Tick += (s, e) =>
            {
                _hoverTimer.Stop();
                if (IslandContainer.IsMouseOver && !_isClickThrough)
                {
                    _toastDismissTimer.Stop();
                    TransitionToState(IslandVisualState.Expanded);
                }
            };

            _leaveTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(260) };
            _leaveTimer.Tick += OnLeaveTick;

            _pickTimer = new DispatcherTimer(DispatcherPriority.Render) { Interval = TimeSpan.FromMilliseconds(30) };
            _pickTimer.Tick += OnPickTick;

            UpdateColorUi(_currentColor);
            UpdateFocusUi();
            UpdateFilesUi();
            UpdateClockText();

            _ready = true;
            Loaded += MainWindow_Loaded;
        }

        private void MainWindow_Loaded(object sender, RoutedEventArgs e)
        {
            CenterOnTopScreen();

            var helper = new WindowInteropHelper(this);
            _hwnd = helper.Handle;
            _hwndSource = HwndSource.FromHwnd(_hwnd);
            if (_hwndSource != null)
            {
                _hwndSource.AddHook(WndProc);
            }

            Win32Interop.ApplyOverlayStyles(_hwnd, false);
            Win32Interop.AddClipboardFormatListener(_hwnd);

            _monitorService.Start();
            _bluetoothService.Start();
            _httpServer.Start();
            ScheduleNextClockTick();
            _waveTimer.Start();
            _topMostWatchdogTimer.Start();
            LoadShelf();

            EnqueueAlert(new IslandEvent(
                "Dynamic Island",
                "Bereit • Maus darüber zum Öffnen",
                IslandEventType.Success,
                null,
                "",
                2600));
        }

        public void CenterOnTopScreen()
        {
            double screenWidth = SystemParameters.PrimaryScreenWidth;
            Left = (screenWidth - Width) / 2.0;
            Top = 0;
        }

        public void SetClickThrough(bool enabled)
        {
            _isClickThrough = enabled;
            Win32Interop.ApplyOverlayStyles(_hwnd, enabled);
            IslandContainer.Opacity = enabled ? 0.80 : 1.0;
        }

        public void ToggleExpandedFromTray()
        {
            if (_currentState == IslandVisualState.Expanded)
            {
                _isPinned = false;
                UpdatePinVisual();
                TransitionToState(IslandVisualState.Compact);
            }
            else
            {
                _isPinned = true;
                UpdatePinVisual();
                TransitionToState(IslandVisualState.Expanded);
            }
        }

        private IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
        {
            if (msg == Win32Interop.WM_CLIPBOARDUPDATE)
            {
                // Text sent from a phone gets its own toast.
                if (DateTime.UtcNow < _ignoreClipboardUntil) return IntPtr.Zero;

                try
                {
                    if (Clipboard.ContainsText())
                    {
                        string text = Clipboard.GetText();
                        _monitorService.NotifyClipboardChanged(text);
                    }
                    else if (Clipboard.ContainsImage())
                    {
                        _monitorService.NotifyClipboardChanged("[Bild in Zwischenablage]");
                    }
                }
                catch
                {
                }
            }
            else if (msg == Win32Interop.WM_DEVICECHANGE)
            {
                int change = wParam.ToInt32();
                if ((change == Win32Interop.DBT_DEVICEARRIVAL || change == Win32Interop.DBT_DEVICEREMOVECOMPLETE) && lParam != IntPtr.Zero)
                {
                    OnVolumeChanged(lParam, change == Win32Interop.DBT_DEVICEARRIVAL);
                }
            }
            return IntPtr.Zero;
        }

        // lParam points to a DEV_BROADCAST_VOLUME: device type at offset 4, drive bit mask at offset 12.
        private void OnVolumeChanged(IntPtr broadcast, bool arrived)
        {
            try
            {
                if (Marshal.ReadInt32(broadcast, 4) != Win32Interop.DBT_DEVTYP_VOLUME) return;

                int unitMask = Marshal.ReadInt32(broadcast, 12);
                for (int i = 0; i < 26; i++)
                {
                    if ((unitMask & (1 << i)) == 0) continue;

                    string drive = (char)('A' + i) + ":";
                    string label = null;
                    if (arrived)
                    {
                        try
                        {
                            label = new System.IO.DriveInfo(drive).VolumeLabel;
                        }
                        catch
                        {
                        }
                    }

                    EnqueueAlert(new IslandEvent(
                        string.IsNullOrWhiteSpace(label) ? "Laufwerk " + drive : label,
                        arrived ? "Laufwerk " + drive + " angeschlossen" : "Laufwerk " + drive + " entfernt",
                        arrived ? IslandEventType.Usb : IslandEventType.Warning,
                        null,
                        arrived ? drive : "OFF",
                        2800));
                }
            }
            catch
            {
            }
        }

        // =====================================================================
        //  Shape spring
        // =====================================================================

        private void SetShapeTarget(double width, double height, double radius)
        {
            _tw = width;
            _th = height;
            _tr = radius;
            if (!_springActive)
            {
                _springActive = true;
                _springLastTime = _springClock.Elapsed.TotalSeconds;
                CompositionTarget.Rendering += OnSpringFrame;
            }
        }

        private void OnSpringFrame(object sender, EventArgs e)
        {
            double now = _springClock.Elapsed.TotalSeconds;
            double dt = Math.Min(0.1, now - _springLastTime);
            _springLastTime = now;
            if (dt <= 0) return;

            int steps = Math.Max(1, (int)Math.Ceiling(dt / 0.004));
            double h = dt / steps;
            for (int i = 0; i < steps; i++)
            {
                SpringStep(ref _w, ref _vw, _tw, h);
                SpringStep(ref _h, ref _vh, _th, h);
                SpringStep(ref _r, ref _vr, _tr, h);
            }

            bool settled =
                Math.Abs(_w - _tw) < 0.3 && Math.Abs(_h - _th) < 0.3 && Math.Abs(_r - _tr) < 0.3 &&
                Math.Abs(_vw) < 3 && Math.Abs(_vh) < 3 && Math.Abs(_vr) < 3;

            if (settled)
            {
                _w = _tw; _h = _th; _r = _tr;
                _vw = _vh = _vr = 0;
                _springActive = false;
                CompositionTarget.Rendering -= OnSpringFrame;
            }

            ApplyShape();
        }

        private static void SpringStep(ref double x, ref double v, double target, double dt)
        {
            double a = -SpringStiffness * (x - target) - SpringDamping * v;
            v += a * dt;
            x += v * dt;
        }

        private void ApplyShape()
        {
            // Even widths keep the centred content from jittering by half pixels.
            double w = Math.Max(2, Math.Round(_w / 2.0) * 2.0);
            double h = Math.Max(2, Math.Round(_h));
            double r = Math.Max(0, Math.Min(_r, h / 2.0));

            IslandContainer.Width = w;
            IslandContainer.Height = h;
            IslandBg.RadiusX = r;
            IslandBg.RadiusY = r;
            IslandStroke.RadiusX = r;
            IslandStroke.RadiusY = r;
            // Push the top rounding above the screen edge: the window cuts it off,
            // so the island has square top corners and looks attached to the edge.
            IslandBg.Margin = new Thickness(0, -r, 0, 0);
            IslandStroke.Margin = new Thickness(0, -r, 0, 0);
            _contentClip.Rect = new Rect(0, -r, w, h + r);
            _contentClip.RadiusX = r;
            _contentClip.RadiusY = r;
        }

        // =====================================================================
        //  State transitions
        // =====================================================================

        private void TransitionToState(IslandVisualState targetState)
        {
            if (targetState != IslandVisualState.Expanded && _picking)
            {
                EndPicking(false);
            }
            if (targetState != IslandVisualState.Expanded && _shareOpen)
            {
                CloseSharePanel();
            }

            bool changed = targetState != _currentState;
            _currentState = targetState;

            if (targetState == IslandVisualState.Toast)
            {
                SetShapeTarget(ToastWidth, ToastHeight, ToastRadius);
            }
            else if (targetState == IslandVisualState.Expanded)
            {
                SetShapeTarget(ExpandedWidth, ExpandedHeight, ExpandedRadius);
            }
            else
            {
                SetShapeTarget(CompactWidth, CompactHeight, CompactRadius);
            }

            if (!changed) return;

            if (targetState == IslandVisualState.Expanded)
            {
                RefreshTelemetryUi(_monitorService.LatestTelemetry);
            }

            AnimateView(CompactView, targetState == IslandVisualState.Compact);
            AnimateView(ToastView, targetState == IslandVisualState.Toast);
            AnimateView(ExpandedView, targetState == IslandVisualState.Expanded);
        }

        // Content melts out (fade + shrink + blur) and the next one resolves in.
        private static void AnimateView(FrameworkElement element, bool visible)
        {
            if (element.IsHitTestVisible == visible) return;
            element.IsHitTestVisible = visible;

            var scale = (ScaleTransform)element.RenderTransform;
            var blur = new BlurEffect { Radius = visible ? 12 : 0, KernelType = KernelType.Gaussian };
            element.Effect = blur;

            if (visible)
            {
                TimeSpan delay = TimeSpan.FromMilliseconds(60);
                TimeSpan duration = TimeSpan.FromMilliseconds(300);

                element.BeginAnimation(OpacityProperty, new DoubleAnimation(1, duration) { BeginTime = delay, EasingFunction = EaseOut });
                var grow = new DoubleAnimation(0.9, 1, duration) { BeginTime = delay, EasingFunction = EaseOut };
                scale.BeginAnimation(ScaleTransform.ScaleXProperty, grow);
                scale.BeginAnimation(ScaleTransform.ScaleYProperty, grow);

                var sharpen = new DoubleAnimation(12, 0, duration) { BeginTime = delay, EasingFunction = EaseOut };
                sharpen.Completed += (s, e) =>
                {
                    if (ReferenceEquals(element.Effect, blur)) element.Effect = null;
                };
                blur.BeginAnimation(BlurEffect.RadiusProperty, sharpen);
            }
            else
            {
                TimeSpan duration = TimeSpan.FromMilliseconds(140);

                element.BeginAnimation(OpacityProperty, new DoubleAnimation(0, duration));
                var shrink = new DoubleAnimation(0.92, duration) { EasingFunction = EaseOut };
                scale.BeginAnimation(ScaleTransform.ScaleXProperty, shrink);
                scale.BeginAnimation(ScaleTransform.ScaleYProperty, shrink);

                var soften = new DoubleAnimation(0, 10, duration);
                soften.Completed += (s, e) =>
                {
                    if (ReferenceEquals(element.Effect, blur)) element.Effect = null;
                };
                blur.BeginAnimation(BlurEffect.RadiusProperty, soften);
            }
        }

        private void ScheduleCollapse(int milliseconds)
        {
            _leaveTimer.Stop();
            _leaveTimer.Interval = TimeSpan.FromMilliseconds(milliseconds);
            _leaveTimer.Start();
        }

        private void OnLeaveTick(object sender, EventArgs e)
        {
            _leaveTimer.Stop();
            if (_currentState != IslandVisualState.Expanded) return;
            if (_isPinned || _holdOpen || _dragOver || _shareOpen) return;

            if (Mouse.Captured != null)
            {
                // A slider drag is still in progress: look again shortly.
                ScheduleCollapse(260);
                return;
            }
            if (IslandContainer.IsMouseOver) return;

            TransitionToState(IslandVisualState.Compact);
        }

        private void IslandContainer_MouseEnter(object sender, MouseEventArgs e)
        {
            if (_isClickThrough) return;
            _leaveTimer.Stop();
            if (_currentState != IslandVisualState.Expanded)
            {
                _hoverTimer.Stop();
                _hoverTimer.Start();
            }
        }

        private void IslandContainer_MouseLeave(object sender, MouseEventArgs e)
        {
            _hoverTimer.Stop();
            ScheduleCollapse(260);
        }

        private void IslandContainer_MouseLeftButtonDown(object sender, MouseButtonEventArgs e)
        {
            // Alt + left-drag moves the island.
            if ((Keyboard.Modifiers & ModifierKeys.Alt) == ModifierKeys.Alt && e.ButtonState == MouseButtonState.Pressed)
            {
                try
                {
                    DragMove();
                }
                catch
                {
                }
            }
        }

        // =====================================================================
        //  Tabs
        // =====================================================================

        private void Tab_Checked(object sender, RoutedEventArgs e)
        {
            if (!_ready) return;
            int index = int.Parse((string)((FrameworkElement)sender).Tag, CultureInfo.InvariantCulture);
            SelectTab(index);
        }

        private void SelectTab(int index)
        {
            if (index == _tabIndex) return;
            if (_picking) EndPicking(false);
            if (_shareOpen) CloseSharePanel();

            int direction = index > _tabIndex ? 1 : -1;
            _tabIndex = index;

            var highlight = (TranslateTransform)TabHighlight.RenderTransform;
            highlight.BeginAnimation(TranslateTransform.XProperty,
                new DoubleAnimation(index * 36.0, TimeSpan.FromMilliseconds(320)) { EasingFunction = new BackEase { EasingMode = EasingMode.EaseOut, Amplitude = 0.35 } });

            for (int i = 0; i < _tabPanels.Length; i++)
            {
                FrameworkElement panel = _tabPanels[i];
                var shift = (TranslateTransform)panel.RenderTransform;
                bool show = i == index;
                panel.IsHitTestVisible = show;

                if (show)
                {
                    TimeSpan duration = TimeSpan.FromMilliseconds(280);
                    panel.BeginAnimation(OpacityProperty, new DoubleAnimation(1, duration) { BeginTime = TimeSpan.FromMilliseconds(50), EasingFunction = EaseOut });
                    shift.BeginAnimation(TranslateTransform.XProperty, new DoubleAnimation(direction * 18.0, 0, duration) { EasingFunction = EaseOut });
                }
                else if (panel.Opacity > 0)
                {
                    TimeSpan duration = TimeSpan.FromMilliseconds(120);
                    panel.BeginAnimation(OpacityProperty, new DoubleAnimation(0, duration));
                    shift.BeginAnimation(TranslateTransform.XProperty, new DoubleAnimation(-direction * 18.0, duration) { EasingFunction = EaseOut });
                }
            }
        }

        private void BtnTogglePin_Click(object sender, RoutedEventArgs e)
        {
            _isPinned = !_isPinned;
            UpdatePinVisual();
            if (!_isPinned && !IslandContainer.IsMouseOver) ScheduleCollapse(260);
        }

        private void UpdatePinVisual()
        {
            PinButton.Content = _isPinned ? "" : "";
            PinButton.Foreground = _isPinned ? (Brush)Brushes.White : (Brush)FindResource("TextSecondary");
            PinButton.Background = (Brush)FindResource(_isPinned ? "AccentGradient" : "CardBrush");
        }

        // =====================================================================
        //  Clock, compact pill, telemetry
        // =====================================================================

        private void OnClockTick(object sender, EventArgs e)
        {
            UpdateClockText();
            ScheduleNextClockTick();
        }

        private void UpdateClockText()
        {
            DateTime now = DateTime.Now;
            string time = now.ToString("HH:mm");
            CompactClockText.Text = time;
            ExpandedClockText.Text = time;
            ExpandedDateText.Text = now.ToString("ddd, d. MMM", German);
        }

        private void ScheduleNextClockTick()
        {
            DateTime now = DateTime.Now;
            int msUntilNextMinute = (60 - now.Second) * 1000 - now.Millisecond + 50;
            _clockTimer.Interval = TimeSpan.FromMilliseconds(Math.Max(200, msUntilNextMinute));
            _clockTimer.Start();
        }

        private void OnWaveTick(object sender, EventArgs e)
        {
            if (_currentState != IslandVisualState.Compact || _focusRunning) return;

            // Bars breathe with CPU load.
            int cpu = _monitorService.LatestTelemetry.CpuPercent;
            double level = Math.Max(0.3, Math.Min(1.0, (cpu + 20) / 90.0));
            AnimateEqBar(EqBar1, level);
            AnimateEqBar(EqBar2, level);
            AnimateEqBar(EqBar3, level);
            AnimateEqBar(EqBar4, level);
        }

        private void AnimateEqBar(FrameworkElement bar, double level)
        {
            double target = 0.22 + _random.NextDouble() * 0.78 * level;
            var scale = (ScaleTransform)bar.RenderTransform;
            scale.BeginAnimation(ScaleTransform.ScaleYProperty,
                new DoubleAnimation(target, TimeSpan.FromMilliseconds(600)) { EasingFunction = SharedSineEase });
        }

        private void UpdateCompactIndicators()
        {
            CompactTimerText.Visibility = _focusRunning ? Visibility.Visible : Visibility.Collapsed;
            CompactBars.Visibility = _focusRunning ? Visibility.Collapsed : Visibility.Visible;
            CompactShelfBadge.Visibility = _shelf.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            CompactShelfText.Text = _shelf.Count.ToString(CultureInfo.InvariantCulture);
        }

        private static void AnimateBar(FrameworkElement fill, double fraction)
        {
            var scale = (ScaleTransform)fill.RenderTransform;
            scale.BeginAnimation(ScaleTransform.ScaleXProperty,
                new DoubleAnimation(Math.Max(0.02, Math.Min(1.0, fraction)), TimeSpan.FromMilliseconds(600)) { EasingFunction = EaseOut });
        }

        private void OnTelemetryUpdated(SystemTelemetrySnapshot snap)
        {
            if (!Dispatcher.CheckAccess())
            {
                Dispatcher.BeginInvoke(new Action(() => OnTelemetryUpdated(snap)));
                return;
            }
            if (_currentState != IslandVisualState.Expanded) return;
            RefreshTelemetryUi(snap);
        }

        private void RefreshTelemetryUi(SystemTelemetrySnapshot snap)
        {
            if (snap == null) return;
            CardCpuText.Text = snap.CpuPercent + " %";
            AnimateBar(CardCpuBar, snap.CpuPercent / 100.0);

            CardRamText.Text = snap.RamPercent + " %";
            CardRamLabel.Text = snap.RamTotalGb > 0
                ? string.Format(German, "RAM · {0:0.0} GB", snap.RamUsedGb)
                : "RAM";
            AnimateBar(CardRamBar, snap.RamPercent / 100.0);

            CardPowerLabel.Text = snap.HasBattery ? (snap.IsCharging ? "Akku · lädt" : "Akku") : "Netzbetrieb";
            CardBatteryText.Text = snap.BatteryPercent + " %";
            AnimateBar(CardBatteryBar, snap.BatteryPercent / 100.0);

            // Sync the volume slider unless the user is dragging it right now.
            if (Mouse.Captured == null)
            {
                _suppressVolumeCallback = true;
                VolumeSlider.Value = snap.VolumePercent;
                _suppressVolumeCallback = false;
                ExpandedVolumeText.Text = snap.VolumePercent.ToString(CultureInfo.InvariantCulture);
            }
            MuteButton.Content = snap.IsMuted ? "" : "";
            MuteButton.Foreground = snap.IsMuted ? (Brush)FindResource("TextSecondary") : Brushes.White;
        }

        // =====================================================================
        //  Toasts
        // =====================================================================

        private void OnStatusAlert(IslandEvent ev)
        {
            Dispatcher.BeginInvoke(new Action(() => EnqueueAlert(ev)));
        }

        public void EnqueueAlert(IslandEvent ev)
        {
            if (ev == null) return;

            Color accent = GetColorForType(ev.Type);
            var accentBrush = new SolidColorBrush(accent);

            ToastIconGlyph.Text = GetGlyphForType(ev.Type);
            ToastIconGlyph.Foreground = accentBrush;
            ToastIconBg.Fill = new SolidColorBrush(Color.FromArgb(46, accent.R, accent.G, accent.B));

            ToastTitleText.Text = ev.Title;
            ToastMessageText.Text = ev.Message;
            ToastBadgeText.Text = ev.BadgeText ?? "";
            ToastBadgeText.Foreground = accentBrush;

            if (ev.ProgressValue.HasValue)
            {
                ToastProgressTrack.Visibility = Visibility.Visible;
                ToastProgressFill.Background = accentBrush;
                AnimateBar(ToastProgressFill, ev.ProgressValue.Value / 100.0);
            }
            else
            {
                ToastProgressTrack.Visibility = Visibility.Collapsed;
            }

            // While the user has the island open, do not pull it away from them.
            if (_currentState != IslandVisualState.Expanded)
            {
                TransitionToState(IslandVisualState.Toast);
                _toastDismissTimer.Stop();
                _toastDismissTimer.Interval = TimeSpan.FromMilliseconds(Math.Max(1400, ev.DurationMs));
                _toastDismissTimer.Start();
            }
        }

        private static Color GetColorForType(IslandEventType type)
        {
            switch (type)
            {
                case IslandEventType.Success:
                    return GreenColor;
                case IslandEventType.Warning:
                    return OrangeColor;
                case IslandEventType.Error:
                    return RedColor;
                default:
                    return AccentColor;
            }
        }

        private static string GetGlyphForType(IslandEventType type)
        {
            switch (type)
            {
                case IslandEventType.Success:
                    return "";
                case IslandEventType.Warning:
                    return "";
                case IslandEventType.Error:
                    return "";
                case IslandEventType.Audio:
                    return "";
                case IslandEventType.Power:
                    return "";
                case IslandEventType.Network:
                    return "";
                case IslandEventType.Clipboard:
                    return "";
                case IslandEventType.KeyLock:
                    return "";
                case IslandEventType.Bluetooth:
                    return "";
                case IslandEventType.Usb:
                    return "";
                case IslandEventType.Timer:
                    return "";
                default:
                    return "";
            }
        }

        // =====================================================================
        //  Media + volume
        // =====================================================================

        private void BtnMediaPrev_Click(object sender, RoutedEventArgs e)
        {
            Win32Interop.SendMediaKey(Win32Interop.VK_MEDIA_PREV_TRACK);
        }

        private void BtnMediaPlayPause_Click(object sender, RoutedEventArgs e)
        {
            Win32Interop.SendMediaKey(Win32Interop.VK_MEDIA_PLAY_PAUSE);
        }

        private void BtnMediaNext_Click(object sender, RoutedEventArgs e)
        {
            Win32Interop.SendMediaKey(Win32Interop.VK_MEDIA_NEXT_TRACK);
        }

        private void VolumeSlider_ValueChanged(object sender, RoutedPropertyChangedEventArgs<double> e)
        {
            if (_suppressVolumeCallback || !IsLoaded) return;
            int vol = (int)Math.Round(e.NewValue);
            CoreAudioInterop.SetVolume(vol);
            ExpandedVolumeText.Text = vol.ToString(CultureInfo.InvariantCulture);
        }

        private void BtnMuteToggle_Click(object sender, RoutedEventArgs e)
        {
            CoreAudioInterop.ToggleMute();
        }

        // =====================================================================
        //  Focus timer
        // =====================================================================

        private void BtnTimerToggle_Click(object sender, RoutedEventArgs e)
        {
            _focusRunning = !_focusRunning;
            if (_focusRunning)
            {
                _focusCountdownTimer.Start();
            }
            else
            {
                _focusCountdownTimer.Stop();
            }
            UpdateFocusUi();
        }

        private void BtnTimerReset_Click(object sender, RoutedEventArgs e)
        {
            ResetFocus(_focusTotalSeconds);
        }

        private void BtnFocusPreset_Click(object sender, RoutedEventArgs e)
        {
            int minutes = int.Parse((string)((FrameworkElement)sender).Tag, CultureInfo.InvariantCulture);
            ResetFocus(minutes * 60);
        }

        private void ResetFocus(int totalSeconds)
        {
            _focusRunning = false;
            _focusCountdownTimer.Stop();
            _focusTotalSeconds = totalSeconds;
            _focusRemainingSeconds = totalSeconds;
            UpdateFocusUi();
        }

        private void OnFocusCountdownTick(object sender, EventArgs e)
        {
            if (_focusRemainingSeconds > 1)
            {
                _focusRemainingSeconds--;
                UpdateFocusUi();
                return;
            }

            int minutes = _focusTotalSeconds / 60;
            ResetFocus(_focusTotalSeconds);
            EnqueueAlert(new IslandEvent(
                "Fokus beendet",
                string.Format("{0} Minuten geschafft", minutes),
                IslandEventType.Timer,
                100,
                "",
                4000));
        }

        private void UpdateFocusUi()
        {
            string text = FormatSeconds(_focusRemainingSeconds);
            FocusTimerText.Text = text;
            CompactTimerText.Text = text;
            FocusToggleButton.Content = _focusRunning ? "Pause" : (_focusRemainingSeconds < _focusTotalSeconds ? "Weiter" : "Start");
            FocusStatusText.Text = _focusRunning
                ? "Läuft"
                : (_focusRemainingSeconds < _focusTotalSeconds ? "Pausiert" : string.Format("{0} Minuten", _focusTotalSeconds / 60));
            UpdateCompactIndicators();

            // Progress ring: arc from 12 o'clock, clockwise, for the remaining share.
            double fraction = _focusTotalSeconds > 0 ? _focusRemainingSeconds / (double)_focusTotalSeconds : 0;
            fraction = Math.Min(0.9999, fraction);
            if (fraction <= 0.0001)
            {
                FocusRingArc.Data = null;
                return;
            }

            const double center = 54, radius = 48;
            double angle = fraction * 2 * Math.PI;
            var start = new Point(center, center - radius);
            var end = new Point(center + radius * Math.Sin(angle), center - radius * Math.Cos(angle));

            var geometry = new StreamGeometry();
            using (StreamGeometryContext ctx = geometry.Open())
            {
                ctx.BeginFigure(start, false, false);
                ctx.ArcTo(end, new Size(radius, radius), 0, fraction > 0.5, SweepDirection.Clockwise, true, false);
            }
            geometry.Freeze();
            FocusRingArc.Data = geometry;
        }

        private static string FormatSeconds(int totalSeconds)
        {
            return string.Format("{0:00}:{1:00}", totalSeconds / 60, totalSeconds % 60);
        }

        // =====================================================================
        //  Colour picker
        // =====================================================================

        private void BtnPipette_Click(object sender, RoutedEventArgs e)
        {
            if (_picking)
            {
                EndPicking(false);
            }
            else
            {
                StartPicking();
            }
        }

        private void StartPicking()
        {
            if (_picking) return;

            if (_loupeBitmap == null)
            {
                _loupeBitmap = new System.Drawing.Bitmap(LoupeSize, LoupeSize, System.Drawing.Imaging.PixelFormat.Format32bppRgb);
                _loupeSource = new WriteableBitmap(LoupeSize, LoupeSize, 96, 96, PixelFormats.Bgr32, null);
                LoupeImage.Source = _loupeSource;
            }

            _mouseProc = MouseHookProc; // field keeps the delegate alive for the native hook
            _mouseHook = Win32Interop.SetWindowsHookEx(Win32Interop.WH_MOUSE_LL, _mouseProc, Win32Interop.GetModuleHandle(null), 0);
            if (_mouseHook == IntPtr.Zero)
            {
                ColorHintText.Text = "Pipette konnte nicht gestartet werden";
                return;
            }

            _picking = true;
            _pickResult = 0;
            _holdOpen = true;
            _colorBeforePick = _currentColor;

            LoupeImage.Visibility = Visibility.Visible;
            LoupeMarker.Visibility = Visibility.Visible;
            PipetteButtonText.Text = "Abbrechen";
            ColorHintText.Text = "Klick wählt · Rechtsklick oder Esc bricht ab";
            _pickTimer.Start();
        }

        private IntPtr MouseHookProc(int nCode, IntPtr wParam, IntPtr lParam)
        {
            if (nCode >= 0 && _picking)
            {
                int msg = wParam.ToInt32();
                if (msg == Win32Interop.WM_LBUTTONDOWN && _pickResult == 0)
                {
                    // Let clicks on the island through (e.g. the cancel button).
                    if (!IslandContainer.IsMouseOver)
                    {
                        Color picked;
                        if (SampleScreen(out picked)) _currentColor = picked;
                        _pickResult = 1;
                        return new IntPtr(1);
                    }
                }
                else if (msg == Win32Interop.WM_RBUTTONDOWN && _pickResult == 0)
                {
                    _pickResult = 2;
                    return new IntPtr(1);
                }
                else if ((msg == Win32Interop.WM_LBUTTONUP || msg == Win32Interop.WM_RBUTTONUP) && _pickResult != 0)
                {
                    bool commit = _pickResult == 1;
                    Dispatcher.BeginInvoke(new Action(() => EndPicking(commit)));
                    return new IntPtr(1);
                }
            }
            return Win32Interop.CallNextHookEx(_mouseHook, nCode, wParam, lParam);
        }

        private void OnPickTick(object sender, EventArgs e)
        {
            if (!_picking || _pickResult != 0) return;

            if ((Win32Interop.GetAsyncKeyState(Win32Interop.VK_ESCAPE) & 0x8000) != 0)
            {
                EndPicking(false);
                return;
            }

            Color live;
            if (SampleScreen(out live))
            {
                UpdateColorUi(live);
            }
        }

        // Grabs the pixels around the cursor into the loupe; returns the centre pixel.
        private bool SampleScreen(out Color center)
        {
            center = Colors.Black;
            try
            {
                Win32Interop.POINT p;
                if (!Win32Interop.GetCursorPos(out p)) return false;

                int half = LoupeSize / 2;
                using (System.Drawing.Graphics g = System.Drawing.Graphics.FromImage(_loupeBitmap))
                {
                    g.CopyFromScreen(p.X - half, p.Y - half, 0, 0, new System.Drawing.Size(LoupeSize, LoupeSize));
                }

                var rect = new System.Drawing.Rectangle(0, 0, LoupeSize, LoupeSize);
                System.Drawing.Imaging.BitmapData data = _loupeBitmap.LockBits(
                    rect, System.Drawing.Imaging.ImageLockMode.ReadOnly, System.Drawing.Imaging.PixelFormat.Format32bppRgb);
                try
                {
                    Marshal.Copy(data.Scan0, _loupeBuffer, 0, _loupeBuffer.Length);
                }
                finally
                {
                    _loupeBitmap.UnlockBits(data);
                }

                _loupeSource.WritePixels(new Int32Rect(0, 0, LoupeSize, LoupeSize), _loupeBuffer, LoupeSize * 4, 0);

                int i = (half * LoupeSize + half) * 4;
                center = Color.FromRgb(_loupeBuffer[i + 2], _loupeBuffer[i + 1], _loupeBuffer[i]);
                return true;
            }
            catch
            {
                return false;
            }
        }

        private void EndPicking(bool commit)
        {
            if (!_picking) return;
            _picking = false;
            _pickTimer.Stop();

            if (_mouseHook != IntPtr.Zero)
            {
                Win32Interop.UnhookWindowsHookEx(_mouseHook);
                _mouseHook = IntPtr.Zero;
            }

            LoupeImage.Visibility = Visibility.Collapsed;
            LoupeMarker.Visibility = Visibility.Collapsed;
            PipetteButtonText.Text = "Pipette";
            _holdOpen = false;

            if (commit)
            {
                UpdateColorUi(_currentColor);
                AddColorToHistory(_currentColor);
                CopyColorText(ToHex(_currentColor), "HEX kopiert");
                // Keep the result on screen for a moment before collapsing.
                if (_currentState == IslandVisualState.Expanded) ScheduleCollapse(3000);
            }
            else
            {
                _currentColor = _colorBeforePick;
                UpdateColorUi(_currentColor);
                ColorHintText.Text = "Wert anklicken zum Kopieren";
                if (_currentState == IslandVisualState.Expanded && !IslandContainer.IsMouseOver) ScheduleCollapse(600);
            }
        }

        private void UpdateColorUi(Color c)
        {
            ColorSwatch.Background = new SolidColorBrush(c);
            HexText.Text = ToHex(c);
            RgbChip.Content = "RGB " + ToRgb(c);
            HslChip.Content = "HSL " + ToHsl(c);
        }

        private void AddColorToHistory(Color c)
        {
            _colorHistory.Remove(c);
            _colorHistory.Insert(0, c);
            if (_colorHistory.Count > 14) _colorHistory.RemoveAt(_colorHistory.Count - 1);

            ColorHistoryPanel.Children.Clear();
            foreach (Color entry in _colorHistory)
            {
                Color captured = entry;
                var dot = new Button
                {
                    Style = (Style)FindResource("SwatchButton"),
                    Background = new SolidColorBrush(captured),
                    ToolTip = ToHex(captured)
                };
                dot.Click += (s, e) =>
                {
                    _currentColor = captured;
                    UpdateColorUi(captured);
                    CopyColorText(ToHex(captured), "HEX kopiert");
                };
                ColorHistoryPanel.Children.Add(dot);
            }

            // The newest dot pops in.
            if (ColorHistoryPanel.Children.Count > 0)
            {
                PopIn((FrameworkElement)ColorHistoryPanel.Children[0]);
            }
        }

        private void BtnCopyHex_Click(object sender, RoutedEventArgs e)
        {
            CopyColorText(ToHex(_currentColor), "HEX kopiert");
        }

        private void BtnCopyRgb_Click(object sender, RoutedEventArgs e)
        {
            CopyColorText("rgb(" + ToRgb(_currentColor) + ")", "RGB kopiert");
        }

        private void BtnCopyHsl_Click(object sender, RoutedEventArgs e)
        {
            CopyColorText("hsl(" + ToHsl(_currentColor).Replace("°", "") + ")", "HSL kopiert");
        }

        private void CopyColorText(string text, string hint)
        {
            ColorHintText.Text = TrySetClipboard(text) ? hint + " · " + text : "Kopieren fehlgeschlagen";
        }

        private static bool TrySetClipboard(string text)
        {
            try
            {
                Clipboard.SetText(text);
                return true;
            }
            catch
            {
                return false;
            }
        }

        private static string ToHex(Color c)
        {
            return string.Format("#{0:X2}{1:X2}{2:X2}", c.R, c.G, c.B);
        }

        private static string ToRgb(Color c)
        {
            return string.Format("{0}, {1}, {2}", c.R, c.G, c.B);
        }

        private static string ToHsl(Color c)
        {
            double r = c.R / 255.0, g = c.G / 255.0, b = c.B / 255.0;
            double max = Math.Max(r, Math.Max(g, b));
            double min = Math.Min(r, Math.Min(g, b));
            double l = (max + min) / 2.0;
            double h = 0, s = 0;
            double d = max - min;

            if (d > 0)
            {
                s = d / (1 - Math.Abs(2 * l - 1));
                if (max == r) h = ((g - b) / d) % 6;
                else if (max == g) h = (b - r) / d + 2;
                else h = (r - g) / d + 4;
                h *= 60;
                if (h < 0) h += 360;
            }

            return string.Format("{0}°, {1}%, {2}%", (int)Math.Round(h) % 360, (int)Math.Round(s * 100), (int)Math.Round(l * 100));
        }

        // =====================================================================
        //  File shelf
        // =====================================================================

        private static bool HasFiles(DragEventArgs e)
        {
            return e.Data.GetDataPresent(DataFormats.FileDrop);
        }

        private void IslandContainer_DragEnter(object sender, DragEventArgs e)
        {
            if (!HasFiles(e) || _isClickThrough) return;

            _dragOver = true;
            _leaveTimer.Stop();
            _hoverTimer.Stop();
            _toastDismissTimer.Stop();
            TabBtnFiles.IsChecked = true;
            TransitionToState(IslandVisualState.Expanded);
            e.Effects = DragDropEffects.Copy;
            e.Handled = true;
        }

        private void IslandContainer_DragOver(object sender, DragEventArgs e)
        {
            e.Effects = HasFiles(e) && !_isClickThrough ? DragDropEffects.Copy : DragDropEffects.None;
            e.Handled = true;
        }

        private void IslandContainer_DragLeave(object sender, DragEventArgs e)
        {
            _dragOver = false;
            ScheduleCollapse(320);
        }

        private void IslandContainer_Drop(object sender, DragEventArgs e)
        {
            _dragOver = false;
            e.Handled = true;
            if (_selfDrag || !HasFiles(e)) return;

            var paths = e.Data.GetData(DataFormats.FileDrop) as string[];
            if (paths == null) return;

            foreach (string path in paths)
            {
                AddToShelf(path);
            }
            UpdateFilesUi();
            SaveShelf();
        }

        private void AddToShelf(string path)
        {
            if (!System.IO.File.Exists(path) && !System.IO.Directory.Exists(path)) return;
            foreach (string existing in _shelf)
            {
                if (string.Equals(existing, path, StringComparison.OrdinalIgnoreCase)) return;
            }

            _shelf.Add(path);
            if (System.IO.File.Exists(path)) _shareServer.Publish(path);
            FrameworkElement chip = CreateFileChip(path);
            FilesPanel.Children.Add(chip);
            PopIn(chip);
            FilesScroll.ScrollToRightEnd();
        }

        private void RemoveFromShelf(string path, UIElement chip)
        {
            _shelf.Remove(path);
            _shareServer.Unshare(path);
            FilesPanel.Children.Remove(chip);
            UpdateFilesUi();
            SaveShelf();
        }

        private void BtnFilesClear_Click(object sender, RoutedEventArgs e)
        {
            foreach (string path in _shelf)
            {
                _shareServer.Unshare(path);
            }
            _shelf.Clear();
            FilesPanel.Children.Clear();
            UpdateFilesUi();
            SaveShelf();
        }

        // The shelf survives restarts: one path per line.
        private void SaveShelf()
        {
            try
            {
                System.IO.Directory.CreateDirectory(System.IO.Path.GetDirectoryName(ShelfStatePath));
                System.IO.File.WriteAllLines(ShelfStatePath, _shelf.ToArray(), System.Text.Encoding.UTF8);
            }
            catch
            {
            }
        }

        private void LoadShelf()
        {
            try
            {
                if (!System.IO.File.Exists(ShelfStatePath)) return;
                foreach (string line in System.IO.File.ReadAllLines(ShelfStatePath, System.Text.Encoding.UTF8))
                {
                    if (line.Trim().Length > 0) AddToShelf(line.Trim());
                }
            }
            catch
            {
            }
            UpdateFilesUi();
        }

        // =====================================================================
        //  Phone share (QR code + hub page)
        // =====================================================================

        private void BtnShareOpen_Click(object sender, RoutedEventArgs e)
        {
            string url;
            try
            {
                url = _shareServer.OpenHub();
            }
            catch
            {
                FilesInfoText.Text = "Freigabe fehlgeschlagen (Port 19846 belegt?)";
                return;
            }

            bool[,] modules = QrCode.Encode(url);
            if (modules == null)
            {
                FilesInfoText.Text = TrySetClipboard(url) ? "Link kopiert · " + url : url;
                return;
            }

            int n = modules.GetLength(0);
            var pixels = new byte[n * n];
            for (int y = 0; y < n; y++)
            {
                for (int x = 0; x < n; x++)
                {
                    pixels[y * n + x] = modules[y, x] ? (byte)0 : (byte)255;
                }
            }
            BitmapSource bitmap = BitmapSource.Create(n, n, 96, 96, PixelFormats.Gray8, null, pixels, n);
            bitmap.Freeze();

            // Whole pixels per module keep the code crisp.
            int modulePixels = Math.Max(1, 110 / n);
            QrImage.Width = n * modulePixels;
            QrImage.Height = n * modulePixels;
            QrImage.Source = bitmap;

            _shareUrl = url;
            _shareOpen = true;
            ShareHintText.Text = ShareDefaultHint;
            FilesSharePanel.Visibility = Visibility.Visible;
            FilesSharePanel.BeginAnimation(OpacityProperty, new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(180)));
        }

        private void BtnShareCopy_Click(object sender, RoutedEventArgs e)
        {
            if (_shareUrl == null) return;
            _ignoreClipboardUntil = DateTime.UtcNow.AddSeconds(1);
            ShareHintText.Text = TrySetClipboard(_shareUrl) ? "Link kopiert · " + _shareUrl : "Kopieren fehlgeschlagen";
        }

        private void BtnShareFolder_Click(object sender, RoutedEventArgs e)
        {
            try
            {
                System.IO.Directory.CreateDirectory(_shareServer.ReceiveFolder);
                Process.Start(_shareServer.ReceiveFolder);
            }
            catch
            {
                ShareHintText.Text = "Ordner konnte nicht geöffnet werden";
            }
        }

        private void BtnShareClose_Click(object sender, RoutedEventArgs e)
        {
            CloseSharePanel();
            if (_currentState == IslandVisualState.Expanded && !IslandContainer.IsMouseOver) ScheduleCollapse(260);
        }

        private void CloseSharePanel()
        {
            _shareOpen = false;
            FilesSharePanel.BeginAnimation(OpacityProperty, null);
            FilesSharePanel.Visibility = Visibility.Collapsed;
        }

        private void OnFileReceived(string path)
        {
            AddToShelf(path);
            UpdateFilesUi();
            SaveShelf();

            string name = System.IO.Path.GetFileName(path);
            FilesInfoText.Text = "Empfangen · " + name;
            ShareHintText.Text = "Empfangen · " + name;
            EnqueueAlert(new IslandEvent("Datei empfangen", name, IslandEventType.Success, 100, "", 3200));
        }

        private void OnTextReceived(string text)
        {
            _ignoreClipboardUntil = DateTime.UtcNow.AddSeconds(1);
            bool copied = TrySetClipboard(text);

            string preview = text.Replace("\r", " ").Replace("\n", " ").Trim();
            if (preview.Length > 38) preview = preview.Substring(0, 38) + "...";

            ShareHintText.Text = copied ? "Text empfangen · in Zwischenablage" : "Text empfangen · Kopieren fehlgeschlagen";
            EnqueueAlert(new IslandEvent("Text vom Handy", preview, IslandEventType.Clipboard, null, copied ? "COPY" : "", 3000));
        }

        private void OnFileDownloaded(string name)
        {
            ShareHintText.Text = "Gesendet · " + name;
            EnqueueAlert(new IslandEvent("Datei gesendet", name, IslandEventType.Success, 100, "", 2600));
        }

        private void UpdateFilesUi()
        {
            int count = _shelf.Count;
            FilesEmptyState.Visibility = count == 0 ? Visibility.Visible : Visibility.Collapsed;
            FilesClearButton.Visibility = count == 0 ? Visibility.Collapsed : Visibility.Visible;
            FilesInfoText.Text = count == 0
                ? "Ablage ist leer"
                : string.Format("{0} {1} · herausziehen oder Link teilen", count, count == 1 ? "Element" : "Elemente");
            UpdateCompactIndicators();
        }

        private FrameworkElement CreateFileChip(string path)
        {
            string name = System.IO.Path.GetFileName(path.TrimEnd('\\', '/'));
            if (string.IsNullOrEmpty(name)) name = path;

            var root = new Grid
            {
                Width = 82,
                Margin = new Thickness(0, 0, 6, 0),
                Background = Brushes.Transparent,
                Cursor = Cursors.Hand,
                ToolTip = path + "\nDoppelklick öffnet · Rechtsklick zeigt im Explorer",
                RenderTransformOrigin = new Point(0.5, 0.5),
                RenderTransform = new ScaleTransform(1, 1)
            };

            root.Children.Add(new Border
            {
                CornerRadius = new CornerRadius(16),
                Background = (Brush)FindResource("CardBrush")
            });

            var stack = new StackPanel { VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(7, 4, 7, 0) };
            stack.Children.Add(new Image
            {
                Width = 32,
                Height = 32,
                Source = GetShellIcon(path),
                HorizontalAlignment = HorizontalAlignment.Center
            });
            stack.Children.Add(new TextBlock
            {
                Text = name,
                FontSize = 10.5,
                Foreground = (Brush)FindResource("TextSecondary"),
                TextAlignment = TextAlignment.Center,
                TextTrimming = TextTrimming.CharacterEllipsis,
                Margin = new Thickness(0, 7, 0, 0)
            });
            root.Children.Add(stack);

            // Hover actions: share link + remove.
            var actions = new StackPanel
            {
                Orientation = Orientation.Horizontal,
                HorizontalAlignment = HorizontalAlignment.Right,
                VerticalAlignment = VerticalAlignment.Top,
                Margin = new Thickness(0, 5, 5, 0),
                Opacity = 0
            };
            var linkButton = new Button
            {
                Style = (Style)FindResource("MiniButton"),
                Content = "",
                ToolTip = "Download-Link fürs lokale Netzwerk kopieren",
                Margin = new Thickness(0, 0, 3, 0)
            };
            var removeButton = new Button
            {
                Style = (Style)FindResource("MiniButton"),
                Content = "",
                ToolTip = "Aus Ablage entfernen"
            };
            linkButton.Click += (s, e) => ShareLink(path);
            removeButton.Click += (s, e) => RemoveFromShelf(path, root);
            actions.Children.Add(linkButton);
            actions.Children.Add(removeButton);
            root.Children.Add(actions);

            root.MouseEnter += (s, e) => actions.BeginAnimation(OpacityProperty, new DoubleAnimation(1, TimeSpan.FromMilliseconds(120)));
            root.MouseLeave += (s, e) => actions.BeginAnimation(OpacityProperty, new DoubleAnimation(0, TimeSpan.FromMilliseconds(200)));

            // Drag the file back out to Explorer, a browser, a chat window …
            root.PreviewMouseLeftButtonDown += (s, e) =>
            {
                bool onButton = IsInsideButton(e.OriginalSource as DependencyObject);
                if (e.ClickCount == 2 && !onButton)
                {
                    _chipDragStart = null;
                    try
                    {
                        Process.Start(path);
                    }
                    catch
                    {
                        FilesInfoText.Text = "Öffnen fehlgeschlagen";
                    }
                    return;
                }
                _chipDragStart = onButton ? (Point?)null : e.GetPosition(this);
            };
            root.MouseRightButtonUp += (s, e) =>
            {
                try
                {
                    Process.Start("explorer.exe", "/select,\"" + path + "\"");
                }
                catch
                {
                }
                e.Handled = true;
            };
            root.MouseMove += (s, e) =>
            {
                if (!_chipDragStart.HasValue || e.LeftButton != MouseButtonState.Pressed) return;
                Point now = e.GetPosition(this);
                if (Math.Abs(now.X - _chipDragStart.Value.X) < 4 && Math.Abs(now.Y - _chipDragStart.Value.Y) < 4) return;

                _chipDragStart = null;
                _holdOpen = true;
                _selfDrag = true;
                try
                {
                    DragDrop.DoDragDrop(root, new DataObject(DataFormats.FileDrop, new[] { path }), DragDropEffects.Copy | DragDropEffects.Link);
                }
                catch
                {
                }
                finally
                {
                    _selfDrag = false;
                    _holdOpen = false;
                    _dragOver = false;
                    ScheduleCollapse(500);
                }
            };

            return root;
        }

        private void ShareLink(string path)
        {
            if (System.IO.Directory.Exists(path))
            {
                FilesInfoText.Text = "Ordner lassen sich nicht verlinken";
                return;
            }

            try
            {
                string url = _shareServer.Share(path);
                FilesInfoText.Text = TrySetClipboard(url) ? "Link kopiert · " + url : url;
            }
            catch
            {
                FilesInfoText.Text = "Freigabe fehlgeschlagen (Port 19846 belegt?)";
            }
        }

        private void FilesScroll_PreviewMouseWheel(object sender, MouseWheelEventArgs e)
        {
            FilesScroll.ScrollToHorizontalOffset(FilesScroll.HorizontalOffset - e.Delta * 0.6);
            e.Handled = true;
        }

        private static bool IsInsideButton(DependencyObject source)
        {
            while (source != null)
            {
                if (source is ButtonBase) return true;
                source = source is Visual ? VisualTreeHelper.GetParent(source) : LogicalTreeHelper.GetParent(source);
            }
            return false;
        }

        private static ImageSource GetShellIcon(string path)
        {
            try
            {
                var info = new Win32Interop.SHFILEINFO();
                IntPtr result = Win32Interop.SHGetFileInfo(path, 0, ref info, (uint)Marshal.SizeOf(typeof(Win32Interop.SHFILEINFO)),
                    Win32Interop.SHGFI_ICON | Win32Interop.SHGFI_LARGEICON);
                if (result == IntPtr.Zero || info.hIcon == IntPtr.Zero) return null;

                try
                {
                    BitmapSource source = Imaging.CreateBitmapSourceFromHIcon(info.hIcon, Int32Rect.Empty, BitmapSizeOptions.FromEmptyOptions());
                    source.Freeze();
                    return source;
                }
                finally
                {
                    Win32Interop.DestroyIcon(info.hIcon);
                }
            }
            catch
            {
                return null;
            }
        }

        private static void PopIn(FrameworkElement element)
        {
            if (!(element.RenderTransform is ScaleTransform))
            {
                element.RenderTransformOrigin = new Point(0.5, 0.5);
                element.RenderTransform = new ScaleTransform(1, 1);
            }
            var scale = (ScaleTransform)element.RenderTransform;
            var pop = new DoubleAnimation(0.6, 1, TimeSpan.FromMilliseconds(360)) { EasingFunction = new BackEase { EasingMode = EasingMode.EaseOut, Amplitude = 0.5 } };
            scale.BeginAnimation(ScaleTransform.ScaleXProperty, pop);
            scale.BeginAnimation(ScaleTransform.ScaleYProperty, pop);
            element.BeginAnimation(OpacityProperty, new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(220)));
        }

        // =====================================================================

        public void Cleanup()
        {
            EndPicking(false);

            _clockTimer.Stop();
            _waveTimer.Stop();
            _toastDismissTimer.Stop();
            _topMostWatchdogTimer.Stop();
            _focusCountdownTimer.Stop();
            _hoverTimer.Stop();
            _leaveTimer.Stop();

            if (_hwnd != IntPtr.Zero)
            {
                Win32Interop.RemoveClipboardFormatListener(_hwnd);
            }

            if (_loupeBitmap != null) _loupeBitmap.Dispose();

            _monitorService.Dispose();
            _bluetoothService.Dispose();
            _httpServer.Dispose();
            _shareServer.Dispose();
        }
    }
}
