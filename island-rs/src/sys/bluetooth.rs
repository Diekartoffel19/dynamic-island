//! Bluetooth connect / disconnect through WinRT device watchers.
//! Event-driven, and covers Bluetooth LE as well as classic devices.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::{Interface, Result, HSTRING};
use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::Win32::Devices::Bluetooth::{
    BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice, BLUETOOTH_DEVICE_INFO,
    BLUETOOTH_DEVICE_SEARCH_PARAMS,
};
use windows_collections::{IIterable, IMapView};

use crate::event::{post, Msg};

const IS_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const DISPLAY_NAME: &str = "System.ItemNameDisplay";

pub struct Watchers {
    _watchers: Vec<DeviceWatcher>,
}

fn lookup_bool(props: &IMapView<HSTRING, windows::core::IInspectable>, key: &str) -> Option<bool> {
    let key = HSTRING::from(key);
    if !props.HasKey(&key).ok()? {
        return None;
    }
    props.Lookup(&key).ok()?.cast::<IPropertyValue>().ok()?.GetBoolean().ok()
}

fn lookup_string(props: &IMapView<HSTRING, windows::core::IInspectable>, key: &str) -> Option<String> {
    let key = HSTRING::from(key);
    if !props.HasKey(&key).ok()? {
        return None;
    }
    Some(props.Lookup(&key).ok()?.cast::<IPropertyValue>().ok()?.GetString().ok()?.to_string())
}

fn watch(selector: &HSTRING) -> Result<DeviceWatcher> {
    let extra: IIterable<HSTRING> = vec![HSTRING::from(IS_CONNECTED)].into();
    let watcher = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(selector, &extra, DeviceInformationKind::AssociationEndpoint)?;

    // Devices listed before the first enumeration ends are the baseline and do not toast.
    let enumerated = Arc::new(AtomicBool::new(false));

    let flag = enumerated.clone();
    watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move |_, info| {
        if let Some(info) = info.as_ref() {
            let connected = info.Properties().ok().and_then(|p| lookup_bool(&p, IS_CONNECTED)).unwrap_or(false);
            post(Msg::BtAdded {
                id: info.Id()?.to_string(),
                name: info.Name()?.to_string(),
                connected,
                initial: !flag.load(Ordering::SeqCst),
            });
        }
        Ok(())
    }))?;

    watcher.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, update| {
        if let Some(update) = update.as_ref() {
            let props = update.Properties().ok();
            post(Msg::BtUpdated {
                id: update.Id()?.to_string(),
                connected: props.as_ref().and_then(|p| lookup_bool(p, IS_CONNECTED)),
                name: props.as_ref().and_then(|p| lookup_string(p, DISPLAY_NAME)),
            });
        }
        Ok(())
    }))?;

    watcher.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, update| {
        if let Some(update) = update.as_ref() {
            post(Msg::BtRemoved { id: update.Id()?.to_string() });
        }
        Ok(())
    }))?;

    watcher.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, windows::core::IInspectable>::new(move |_, _| {
        enumerated.store(true, Ordering::SeqCst);
        Ok(())
    }))?;

    watcher.Start()?;
    Ok(watcher)
}

/// Starts watching paired devices. Keep the result alive.
pub fn start() -> Watchers {
    let mut watchers = Vec::new();
    if let Ok(selector) = BluetoothDevice::GetDeviceSelectorFromPairingState(true) {
        if let Ok(w) = watch(&selector) {
            watchers.push(w);
        }
    }
    if let Ok(selector) = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true) {
        if let Ok(w) = watch(&selector) {
            watchers.push(w);
        }
    }
    Watchers { _watchers: watchers }
}

impl Drop for Watchers {
    fn drop(&mut self) {
        for w in &self._watchers {
            let _ = w.Stop();
        }
    }
}

/// The device address is the tail of the endpoint id: "...-aa:bb:cc:dd:ee:ff".
pub fn address_from_id(id: &str) -> Option<u64> {
    let tail = id.rsplit('-').next()?;
    let hex: String = tail.chars().filter(|c| *c != ':').collect();
    if hex.len() != 12 {
        return None;
    }
    u64::from_str_radix(&hex, 16).ok()
}

/// Class of device of a paired classic device, 0 if unknown (all LE-only devices).
fn class_of_device(address: u64) -> u32 {
    unsafe {
        let search = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnAuthenticated: true.into(),
            fReturnRemembered: true.into(),
            fReturnConnected: true.into(),
            ..Default::default()
        };
        let mut info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
        let Ok(find) = BluetoothFindFirstDevice(&search, &mut info) else { return 0 };
        let mut class = 0;
        loop {
            if info.Address.Anonymous.ullLong == address {
                class = info.ulClassofDevice;
                break;
            }
            info = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
            if BluetoothFindNextDevice(find, &mut info).is_err() {
                break;
            }
        }
        let _ = BluetoothFindDeviceClose(find);
        class
    }
}

/// German device kind from the Bluetooth class of device (major class: bits 8..12).
pub fn describe(address: Option<u64>) -> &'static str {
    let class = address.map(class_of_device).unwrap_or(0);
    let major = (class >> 8) & 0x1F;
    let minor = (class >> 2) & 0x3F;
    match major {
        1 => "Computer",
        2 => "Handy",
        4 => {
            if minor == 5 || minor == 6 {
                "Lautsprecher"
            } else {
                "Kopfhörer"
            }
        }
        5 => {
            if minor & 0x30 == 0x10 {
                "Tastatur"
            } else if minor & 0x30 == 0x20 {
                "Maus"
            } else if minor & 0x0F == 1 || minor & 0x0F == 2 {
                "Controller"
            } else {
                "Eingabegerät"
            }
        }
        6 => "Drucker",
        7 => "Wearable",
        _ => "Bluetooth-Gerät",
    }
}
