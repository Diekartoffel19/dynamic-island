//! Network state. The IP helper calls back when an interface or address changes.

use std::net::Ipv4Addr;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, NotifyIpInterfaceChange, NotifyUnicastIpAddressChange, GAA_FLAG_INCLUDE_GATEWAYS,
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH, MIB_IPINTERFACE_ROW,
    MIB_NOTIFICATION_TYPE, MIB_UNICASTIPADDRESS_ROW,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};

use crate::event::{post, Msg};

const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_TUNNEL: u32 = 131;

unsafe extern "system" fn on_interface_change(_ctx: *const core::ffi::c_void, _row: *const MIB_IPINTERFACE_ROW, _kind: MIB_NOTIFICATION_TYPE) {
    post(Msg::NetworkChanged);
}

unsafe extern "system" fn on_address_change(_ctx: *const core::ffi::c_void, _row: *const MIB_UNICASTIPADDRESS_ROW, _kind: MIB_NOTIFICATION_TYPE) {
    post(Msg::NetworkChanged);
}

pub fn register() {
    unsafe {
        let mut handle = HANDLE::default();
        let _ = NotifyIpInterfaceChange(AF_UNSPEC, Some(on_interface_change), None, false, &mut handle);
        let mut handle = HANDLE::default();
        let _ = NotifyUnicastIpAddressChange(AF_UNSPEC, Some(on_address_change), None, false, &mut handle);
    }
}

struct Adapter {
    name: String,
    /// Address and on-link prefix length.
    ipv4: Vec<(Ipv4Addr, u8)>,
    has_gateway: bool,
}

/// Adapters that are up, without loopback and tunnels.
fn adapters() -> Vec<Adapter> {
    let mut out = Vec::new();
    unsafe {
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 16 * 1024;
        let mut buffer: Vec<u64> = Vec::new();
        for _ in 0..3 {
            buffer = vec![0u64; size as usize / 8 + 1];
            let result = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size);
            if result == 0 {
                break;
            }
            if result != 111 {
                // Anything but ERROR_BUFFER_OVERFLOW: give up.
                return out;
            }
        }

        let mut current = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !current.is_null() {
            let adapter = &*current;
            current = adapter.Next;
            if adapter.OperStatus != IfOperStatusUp || adapter.IfType == IF_TYPE_SOFTWARE_LOOPBACK || adapter.IfType == IF_TYPE_TUNNEL {
                continue;
            }

            let mut ipv4 = Vec::new();
            let mut unicast = adapter.FirstUnicastAddress;
            while !unicast.is_null() {
                let entry = &*unicast;
                unicast = entry.Next;
                let sockaddr = entry.Address.lpSockaddr;
                if !sockaddr.is_null() && (*sockaddr).sa_family == AF_INET {
                    let v4 = &*(sockaddr as *const SOCKADDR_IN);
                    let ip = Ipv4Addr::from(u32::from_be(v4.sin_addr.S_un.S_addr));
                    if !ip.is_loopback() {
                        ipv4.push((ip, entry.OnLinkPrefixLength));
                    }
                }
            }

            out.push(Adapter {
                name: adapter.FriendlyName.to_string().unwrap_or_default(),
                ipv4,
                has_gateway: !adapter.FirstGatewayAddress.is_null(),
            });
        }
    }
    out
}

/// Online flag and the name of the first active adapter.
pub fn status() -> (bool, String) {
    let list = adapters();
    match list.first() {
        Some(first) => (true, if first.name.is_empty() { "Online".to_string() } else { first.name.clone() }),
        None => (false, "Offline".to_string()),
    }
}

/// IPv4 address other devices on the local network can reach.
pub fn local_address() -> String {
    let list = adapters();
    for adapter in &list {
        if adapter.has_gateway {
            if let Some((ip, _)) = adapter.ipv4.first() {
                return ip.to_string();
            }
        }
    }
    for adapter in &list {
        if let Some((ip, _)) = adapter.ipv4.first() {
            return ip.to_string();
        }
    }
    "127.0.0.1".to_string()
}

/// Directed broadcast address of every active IPv4 network, plus 255.255.255.255.
/// The limited broadcast alone often leaves through one adapter only.
pub fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut out = vec![Ipv4Addr::BROADCAST];
    for adapter in adapters() {
        for (ip, prefix) in adapter.ipv4 {
            // Link-local (169.254/16) addresses only exist while DHCP failed.
            if ip.is_link_local() || prefix == 0 || prefix >= 31 {
                continue;
            }
            let mask = u32::MAX << (32 - prefix as u32);
            let broadcast = Ipv4Addr::from(u32::from(ip) | !mask);
            if !out.contains(&broadcast) {
                out.push(broadcast);
            }
        }
    }
    out
}

/// Name of the Wi-Fi network and its signal (0..5 bars), if the internet connection is Wi-Fi.
/// The profile name is the SSID for networks joined from the Windows UI, and needs no location access.
fn wifi_info() -> Option<(String, u8)> {
    use windows::Networking::Connectivity::NetworkInformation;
    let profile = NetworkInformation::GetInternetConnectionProfile().ok()?;
    if !profile.IsWlanConnectionProfile().ok()? {
        return None;
    }
    let name = profile.ProfileName().ok()?.to_string();
    let bars = profile.GetSignalBars().ok().and_then(|r| r.Value().ok()).unwrap_or(0);
    Some((name, bars))
}

/// Looks up the Wi-Fi details on a worker thread and posts them as `Msg::Wifi`.
pub fn query_wifi() {
    let _ = std::thread::Builder::new().name("IslandWifi".into()).stack_size(256 * 1024).spawn(|| unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
        let ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        post(Msg::Wifi(wifi_info()));
        if ok {
            CoUninitialize();
        }
    });
}
