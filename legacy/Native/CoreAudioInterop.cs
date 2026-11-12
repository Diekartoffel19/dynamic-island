using System;
using System.Runtime.InteropServices;

namespace DynamicIsland.Native
{
    public static class CoreAudioInterop
    {
        [ComImport]
        [Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
        private class MMDeviceEnumeratorComObject
        {
        }

        private enum EDataFlow
        {
            eRender = 0,
            eCapture = 1,
            eAll = 2
        }

        private enum ERole
        {
            eConsole = 0,
            eMultimedia = 1,
            eCommunications = 2
        }

        [Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IMMDeviceEnumerator
        {
            int EnumAudioEndpoints(EDataFlow dataFlow, int dwStateMask, out IntPtr ppDevices);
            int GetDefaultAudioEndpoint(EDataFlow dataFlow, ERole role, out IMMDevice ppEndpoint);
        }

        [Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IMMDevice
        {
            int Activate(ref Guid iid, int dwClsCtx, IntPtr pActivationParams, [MarshalAs(UnmanagedType.IUnknown)] out object ppInterface);
        }

        [Guid("657804FA-D6AD-4496-8A60-352752AF4F89"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IAudioEndpointVolumeCallback
        {
            [PreserveSig]
            int OnNotify(IntPtr pNotify);
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct AUDIO_VOLUME_NOTIFICATION_DATA
        {
            public Guid guidEventContext;
            [MarshalAs(UnmanagedType.Bool)]
            public bool bMuted;
            public float fMasterVolume;
            public uint nChannels;
        }

        private sealed class VolumeCallback : IAudioEndpointVolumeCallback
        {
            private readonly Action<int, bool> _onChanged;

            public VolumeCallback(Action<int, bool> onChanged)
            {
                _onChanged = onChanged;
            }

            public int OnNotify(IntPtr pNotify)
            {
                if (pNotify != IntPtr.Zero && _onChanged != null)
                {
                    try
                    {
                        var data = (AUDIO_VOLUME_NOTIFICATION_DATA)Marshal.PtrToStructure(
                            pNotify, typeof(AUDIO_VOLUME_NOTIFICATION_DATA));
                        int vol = (int)Math.Round(data.fMasterVolume * 100.0f);
                        _onChanged(vol, data.bMuted);
                    }
                    catch
                    {
                    }
                }
                return 0;
            }
        }

        [Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IAudioEndpointVolume
        {
            int RegisterControlChangeNotify(IAudioEndpointVolumeCallback pNotify);
            int UnregisterControlChangeNotify(IAudioEndpointVolumeCallback pNotify);
            int GetChannelCount(out int pnChannelCount);
            int SetMasterVolumeLevel(float fLevelDB, ref Guid pguidEventContext);
            int SetMasterVolumeLevelScalar(float fLevel, ref Guid pguidEventContext);
            int GetMasterVolumeLevel(out float pfLevelDB);
            int GetMasterVolumeLevelScalar(out float pfLevel);
            int SetChannelVolumeLevel(uint nChannel, float fLevelDB, ref Guid pguidEventContext);
            int SetChannelVolumeLevelScalar(uint nChannel, float fLevel, ref Guid pguidEventContext);
            int GetChannelVolumeLevel(uint nChannel, out float pfLevelDB);
            int GetChannelVolumeLevelScalar(uint nChannel, out float pfLevel);
            int SetMute([MarshalAs(UnmanagedType.Bool)] bool bMute, ref Guid pguidEventContext);
            int GetMute(out bool pbMute);
        }

        private static IAudioEndpointVolume _registeredEndpoint;
        private static VolumeCallback _registeredCallback;

        public static void RegisterVolumeCallback(Action<int, bool> onChanged)
        {
            UnregisterVolumeCallback();
            try
            {
                var ep = GetVolumeInterface();
                if (ep == null) return;
                var cb = new VolumeCallback(onChanged);
                if (ep.RegisterControlChangeNotify(cb) == 0)
                {
                    _registeredEndpoint = ep;
                    _registeredCallback = cb;
                }
            }
            catch
            {
            }
        }

        public static void UnregisterVolumeCallback()
        {
            try
            {
                if (_registeredEndpoint != null && _registeredCallback != null)
                {
                    _registeredEndpoint.UnregisterControlChangeNotify(_registeredCallback);
                }
            }
            catch
            {
            }
            _registeredEndpoint = null;
            _registeredCallback = null;
        }

        private static IAudioEndpointVolume GetVolumeInterface()
        {
            try
            {
                var enumerator = new MMDeviceEnumeratorComObject() as IMMDeviceEnumerator;
                if (enumerator == null) return null;

                IMMDevice device;
                int hr = enumerator.GetDefaultAudioEndpoint(EDataFlow.eRender, ERole.eMultimedia, out device);
                if (hr != 0 || device == null) return null;

                Guid iid = typeof(IAudioEndpointVolume).GUID;
                object obj;
                hr = device.Activate(ref iid, 23, IntPtr.Zero, out obj);
                if (hr != 0 || obj == null) return null;

                return obj as IAudioEndpointVolume;
            }
            catch
            {
                return null;
            }
        }

        public static bool TryGetVolume(out int volumePercent, out bool isMuted)
        {
            volumePercent = 50;
            isMuted = false;
            try
            {
                var vol = GetVolumeInterface();
                if (vol == null) return false;

                float level;
                bool mute;
                vol.GetMasterVolumeLevelScalar(out level);
                vol.GetMute(out mute);

                volumePercent = (int)Math.Round(level * 100.0f);
                isMuted = mute;
                return true;
            }
            catch
            {
                return false;
            }
        }

        public static void SetVolume(int volumePercent)
        {
            try
            {
                var vol = GetVolumeInterface();
                if (vol == null) return;
                float scalar = Math.Max(0f, Math.Min(1f, volumePercent / 100f));
                Guid ctx = Guid.Empty;
                vol.SetMasterVolumeLevelScalar(scalar, ref ctx);
            }
            catch
            {
            }
        }

        public static void ToggleMute()
        {
            try
            {
                var vol = GetVolumeInterface();
                if (vol == null) return;
                bool mute;
                vol.GetMute(out mute);
                Guid ctx = Guid.Empty;
                vol.SetMute(!mute, ref ctx);
            }
            catch
            {
            }
        }
    }
}
