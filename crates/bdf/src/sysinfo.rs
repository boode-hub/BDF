//! Process memory and CPU figures, for the performance overlay and the
//! built-in benchmark. Missing values are `None`, never zero.

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcStats {
    pub rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    /// User + kernel CPU time since process start.
    pub cpu_secs: Option<f64>,
}

#[cfg(target_os = "linux")]
pub fn sample() -> ProcStats {
    let mut s = ProcStats::default();
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            let kb = |l: &str| l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).map(|v| v * 1024);
            if line.starts_with("VmRSS:") {
                s.rss_bytes = kb(line);
            } else if line.starts_with("VmHWM:") {
                s.peak_rss_bytes = kb(line);
            }
        }
    }
    if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
        // Fields after the command name (which may contain spaces).
        if let Some(rest) = stat.rsplit(')').next() {
            let f: Vec<&str> = rest.split_whitespace().collect();
            // utime and stime are fields 14 and 15 overall: 12 and 13 here.
            if let (Some(u), Some(k)) = (f.get(11).and_then(|v| v.parse::<f64>().ok()), f.get(12).and_then(|v| v.parse::<f64>().ok())) {
                // SAFETY: sysconf has no preconditions.
                let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
                if tck > 0 {
                    s.cpu_secs = Some((u + k) / tck as f64);
                }
            }
        }
    }
    s
}

#[cfg(target_os = "macos")]
pub fn sample() -> ProcStats {
    let mut s = ProcStats::default();
    // SAFETY: getrusage fills the zeroed struct.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0 {
        // ru_maxrss is in bytes on macOS.
        s.peak_rss_bytes = Some(ru.ru_maxrss as u64);
        let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
        s.cpu_secs = Some(t(ru.ru_utime) + t(ru.ru_stime));
    }
    s
}

#[cfg(windows)]
pub fn sample() -> ProcStats {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut s = ProcStats::default();
    // SAFETY: the pseudo-handle needs no closing; out-structs are sized correctly.
    unsafe {
        let proc_ = GetCurrentProcess();
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if K32GetProcessMemoryInfo(proc_, &mut pmc, pmc.cb) != 0 {
            s.rss_bytes = Some(pmc.WorkingSetSize as u64);
            s.peak_rss_bytes = Some(pmc.PeakWorkingSetSize as u64);
        }
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        if GetProcessTimes(proc_, &mut c, &mut e, &mut k, &mut u) != 0 {
            let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
            s.cpu_secs = Some(ft(k) + ft(u));
        }
    }
    s
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn sample() -> ProcStats {
    ProcStats::default()
}

pub fn mib(b: Option<u64>) -> String {
    match b {
        Some(b) => format!("{:.0} MB", b as f64 / 1_048_576.0),
        None => "n/a".into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn reports_memory_and_cpu() {
        let s = super::sample();
        assert!(s.rss_bytes.unwrap() > 1_000_000);
        assert!(s.peak_rss_bytes.unwrap() >= s.rss_bytes.unwrap());
        assert!(s.cpu_secs.is_some());
    }
}
