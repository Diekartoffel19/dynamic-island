//! CPU and RAM load. Sampled on a timer that only runs while the values are on screen.

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

fn to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

#[derive(Default)]
pub struct CpuSampler {
    idle: u64,
    kernel: u64,
    user: u64,
}

impl CpuSampler {
    pub fn new() -> Self {
        let mut s = CpuSampler::default();
        s.sample();
        s
    }

    /// Load in percent since the previous call.
    pub fn sample(&mut self) -> Option<i32> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
        let (idle, kernel, user) = (to_u64(idle), to_u64(kernel), to_u64(user));

        let idle_diff = idle.wrapping_sub(self.idle);
        let total = kernel.wrapping_sub(self.kernel) + user.wrapping_sub(self.user);
        self.idle = idle;
        self.kernel = kernel;
        self.user = user;

        if total == 0 {
            return None;
        }
        let busy = total.saturating_sub(idle_diff) as f64;
        Some(((busy * 100.0 / total as f64).round() as i32).clamp(0, 100))
    }
}

pub struct Memory {
    pub percent: i32,
    pub used_gb: f64,
    pub total_gb: f64,
}

pub fn memory() -> Option<Memory> {
    let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut mem).ok()? };
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    Some(Memory {
        percent: mem.dwMemoryLoad as i32,
        used_gb: ((mem.ullTotalPhys - mem.ullAvailPhys) as f64 / GB * 10.0).round() / 10.0,
        total_gb: (mem.ullTotalPhys as f64 / GB * 10.0).round() / 10.0,
    })
}
