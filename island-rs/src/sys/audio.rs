//! Master volume of the default output device.
//! Changes arrive through IAudioEndpointVolumeCallback, not a poll.

use windows::core::{implement, Result, GUID, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl, IAudioMeterInformation,
};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::event::{post, Msg};

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback;

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if let Some(data) = unsafe { data.as_ref() } {
            post(Msg::Volume { percent: (data.fMasterVolume * 100.0).round() as i32, muted: data.bMuted.as_bool() });
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceCallback;

impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eMultimedia {
            post(Msg::AudioDeviceChanged);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

pub struct Audio {
    enumerator: IMMDeviceEnumerator,
    volume: Option<IAudioEndpointVolume>,
    /// Output level of the device, to tell whether something is playing.
    meter: Option<IAudioMeterInformation>,
    callback: IAudioEndpointVolumeCallback,
    device_callback: IMMNotificationClient,
}

impl Audio {
    pub fn new() -> Option<Audio> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device_callback: IMMNotificationClient = DeviceCallback.into();
            let _ = enumerator.RegisterEndpointNotificationCallback(&device_callback);
            let mut audio = Audio { enumerator, volume: None, meter: None, callback: VolumeCallback.into(), device_callback };
            audio.rebind();
            Some(audio)
        }
    }

    /// Follows the default output device. Call again when it changes.
    pub fn rebind(&mut self) {
        unsafe {
            if let Some(old) = self.volume.take() {
                let _ = old.UnregisterControlChangeNotify(&self.callback);
            }
            self.meter = None;
            let Ok(device) = self.enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) else { return };
            self.meter = device.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).ok();
            let Ok(volume) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else { return };
            let _ = volume.RegisterControlChangeNotify(&self.callback);
            self.volume = Some(volume);
        }
    }

    /// Volume in percent and the mute flag.
    pub fn get(&self) -> Option<(i32, bool)> {
        let volume = self.volume.as_ref()?;
        unsafe {
            let level = volume.GetMasterVolumeLevelScalar().ok()?;
            let muted = volume.GetMute().ok()?.as_bool();
            Some(((level * 100.0).round() as i32, muted))
        }
    }

    /// Current peak output level, 0..1. Above zero only while something plays.
    pub fn peak(&self) -> f32 {
        self.meter.as_ref().and_then(|m| unsafe { m.GetPeakValue().ok() }).unwrap_or(0.0)
    }

    pub fn set_volume(&self, percent: i32) {
        if let Some(volume) = &self.volume {
            unsafe {
                let _ = volume.SetMasterVolumeLevelScalar((percent as f32 / 100.0).clamp(0.0, 1.0), &GUID::zeroed());
            }
        }
    }

    pub fn toggle_mute(&self) {
        if let Some(volume) = &self.volume {
            unsafe {
                if let Ok(muted) = volume.GetMute() {
                    let _ = volume.SetMute(!muted.as_bool(), &GUID::zeroed());
                }
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe {
            if let Some(volume) = &self.volume {
                let _ = volume.UnregisterControlChangeNotify(&self.callback);
            }
            let _ = self.enumerator.UnregisterEndpointNotificationCallback(&self.device_callback);
        }
    }
}
