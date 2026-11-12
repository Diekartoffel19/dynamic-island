//! Battery and power line. Changes arrive as WM_POWERBROADCAST.

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Power::{GetSystemPowerStatus, RegisterPowerSettingNotification, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemServices::{GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_WINDOW_HANDLE;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PowerState {
    pub has_battery: bool,
    pub percent: i32,
    /// 0 = offline, 1 = online, 255 = unknown.
    pub line: u8,
}

pub fn read() -> PowerState {
    let mut st = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut st) }.is_err() {
        return PowerState { has_battery: false, percent: 100, line: 255 };
    }
    let has_battery = st.BatteryFlag != 255 && st.BatteryFlag & 128 == 0;
    let percent = if has_battery && st.BatteryLifePercent <= 100 { st.BatteryLifePercent as i32 } else { 100 };
    PowerState { has_battery, percent, line: st.ACLineStatus }
}

/// Asks for WM_POWERBROADCAST / PBT_POWERSETTINGCHANGE on line and charge changes.
pub fn register(hwnd: HWND) {
    unsafe {
        let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_ACDC_POWER_SOURCE, DEVICE_NOTIFY_WINDOW_HANDLE);
        let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_BATTERY_PERCENTAGE_REMAINING, DEVICE_NOTIFY_WINDOW_HANDLE);
    }
}
