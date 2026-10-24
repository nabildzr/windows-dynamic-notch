//! CPU, memory, system drive and network throughput for the System widget.
//!
//! Sampled once a second, and only while the widget is on screen: the UI
//! switches sampling on and off through a watch channel.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::core::w;

use super::{EventBus, ServiceEvent};

/// CPU history length (seconds) shown as a sparkline.
pub const HISTORY: usize = 48;

#[derive(Clone, Debug, Default)]
pub struct SystemStats {
    /// 0..1
    pub cpu: f32,
    pub memory: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disk: f32,
    pub disk_free: u64,
    pub disk_total: u64,
    /// Bytes per second.
    pub download: f64,
    pub upload: f64,
    /// Recent CPU samples, oldest first.
    pub cpu_history: VecDeque<f32>,
    /// At least one full sample was taken.
    pub ready: bool,
}

pub struct SystemHandle {
    active: watch::Sender<bool>,
}

impl SystemHandle {
    /// Starts or stops sampling (only while the widget is visible).
    pub fn set_active(&self, active: bool) {
        self.active.send_if_modified(|current| {
            let changed = *current != active;
            *current = active;
            changed
        });
    }
}

fn filetime(ft: FILETIME) -> u64 {
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

fn cpu_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
    // Kernel time includes idle time.
    Some((filetime(idle), filetime(kernel) + filetime(user)))
}

/// Total received / sent bytes over physical interfaces.
fn network_bytes() -> Option<(u64, u64)> {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        GetIfTable2(&mut table).ok().ok()?;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let (mut down, mut up) = (0u64, 0u64);
        for row in rows {
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            // HardwareInterface set, FilterInterface clear: count each NIC once.
            if flags & 1 != 0 && flags & 2 == 0 {
                down += row.InOctets;
                up += row.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        Some((down, up))
    }
}

fn memory() -> Option<(u64, u64)> {
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut status).ok()? };
    Some((status.ullTotalPhys - status.ullAvailPhys, status.ullTotalPhys))
}

fn system_drive() -> Option<(u64, u64)> {
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(w!("C:\\"), None, Some(&mut total), Some(&mut free)).ok()? };
    Some((free, total))
}

struct Sampler {
    stats: SystemStats,
    cpu: Option<(u64, u64)>,
    net: Option<((u64, u64), Instant)>,
}

impl Sampler {
    fn sample(&mut self) {
        let stats = &mut self.stats;
        if let Some((idle, total)) = cpu_times() {
            if let Some((last_idle, last_total)) = self.cpu {
                let busy = (total - last_total).saturating_sub(idle - last_idle);
                let span = (total - last_total).max(1);
                stats.cpu = (busy as f64 / span as f64).clamp(0.0, 1.0) as f32;
                stats.cpu_history.push_back(stats.cpu);
                while stats.cpu_history.len() > HISTORY {
                    stats.cpu_history.pop_front();
                }
                stats.ready = true;
            }
            self.cpu = Some((idle, total));
        }
        if let Some(bytes) = network_bytes() {
            let now = Instant::now();
            if let Some(((down, up), at)) = self.net {
                let seconds = now.duration_since(at).as_secs_f64().max(0.2);
                stats.download = bytes.0.saturating_sub(down) as f64 / seconds;
                stats.upload = bytes.1.saturating_sub(up) as f64 / seconds;
            }
            self.net = Some((bytes, now));
        }
        if let Some((used, total)) = memory() {
            stats.memory_used = used;
            stats.memory_total = total;
            stats.memory = (used as f64 / total.max(1) as f64) as f32;
        }
        if let Some((free, total)) = system_drive() {
            stats.disk_free = free;
            stats.disk_total = total;
            stats.disk = (1.0 - free as f64 / total.max(1) as f64) as f32;
        }
    }
}

pub fn spawn(rt: &Handle, bus: EventBus) -> SystemHandle {
    // Free space of C: for the disk-full warning, every 10 minutes.
    let disk_bus = bus.clone();
    rt.spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some((free, total)) = system_drive() {
                disk_bus.send(ServiceEvent::Disk { free, total });
            }
        }
    });
    let (active, mut rx) = watch::channel(false);
    rt.spawn(async move {
        let mut sampler = Sampler { stats: SystemStats::default(), cpu: None, net: None };
        loop {
            if !*rx.borrow_and_update() {
                // Deltas across a pause would be meaningless.
                sampler.cpu = None;
                sampler.net = None;
                if rx.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        sampler.sample();
                        bus.send(ServiceEvent::System(Box::new(sampler.stats.clone())));
                    }
                    changed = rx.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        if !*rx.borrow() {
                            break;
                        }
                    }
                }
            }
        }
    });
    SystemHandle { active }
}

/// "12.3 MB/s", "840 KB/s", "0 B/s".
pub fn format_rate(bytes_per_second: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bytes_per_second.max(0.0);
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Gigabytes with one decimal ("7.4").
pub fn gigabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_rates() {
        assert_eq!(format_rate(0.0), "0 B/s");
        assert_eq!(format_rate(512.0), "512 B/s");
        assert_eq!(format_rate(2048.0), "2.0 KB/s");
        assert_eq!(format_rate(5.5 * 1024.0 * 1024.0), "5.5 MB/s");
        assert_eq!(format_rate(200.0 * 1024.0), "200 KB/s");
    }
}
