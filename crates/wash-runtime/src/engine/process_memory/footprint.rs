//! Platform memory acquisition, separate from guard policy and sampling.

/// This process's memory footprint, in bytes, counting pages the kernel has
/// swapped or compressed. `None` where it cannot be read.
pub fn process_footprint() -> Option<u64> {
    FootprintReader::new().read()
}

/// Reads the footprint over and over without paying to set up each reading.
///
/// On Linux it keeps `/proc/self/status` open and re-reads it from the start
/// into one buffer: no open, and so no `EMFILE` on a host at its descriptor
/// limit, and no allocation per sample.
///
/// Elsewhere `sysinfo` keeps one `System` for the life of the reader rather
/// than building one per sample.
pub(super) struct FootprintReader {
    #[cfg(target_os = "linux")]
    status: Option<std::fs::File>,
    #[cfg(target_os = "linux")]
    buf: String,
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    system: sysinfo::System,
}

impl FootprintReader {
    pub(super) fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            status: None,
            #[cfg(target_os = "linux")]
            buf: String::with_capacity(2048),
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            system: sysinfo::System::new_with_specifics(sysinfo::RefreshKind::new()),
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn read(&mut self) -> Option<u64> {
        use std::io::{Read as _, Seek as _};

        let status = match &mut self.status {
            Some(status) => status,
            None => self
                .status
                .insert(std::fs::File::open("/proc/self/status").ok()?),
        };
        self.buf.clear();
        if status.rewind().is_err() || status.read_to_string(&mut self.buf).is_err() {
            // Reopened next time rather than trusted again.
            self.status = None;
            return None;
        }
        linux_footprint(&self.buf)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn read(&mut self) -> Option<u64> {
        read_footprint()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn read(&mut self) -> Option<u64> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};

        let pid = sysinfo::get_current_pid().ok()?;
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::new().with_memory(),
        );
        self.system.process(pid).map(|process| process.memory())
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn read_footprint() -> Option<u64> {
    let pid = libc::c_int::try_from(std::process::id()).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: `RUSAGE_INFO_V2` tells the kernel to write one
    // `rusage_info_v2`, which is exactly what the buffer holds, for a pid that
    // is our own and therefore alive.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, info.as_mut_ptr().cast()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: a zero return means the kernel filled the buffer, and it was
    // zeroed before that, so every field is initialised either way.
    Some(unsafe { info.assume_init() }.ri_phys_footprint)
}

/// `VmRSS + VmSwap` out of `/proc/self/status`, in bytes.
#[cfg(any(target_os = "linux", test))]
pub(super) fn linux_footprint(status: &str) -> Option<u64> {
    let kib = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|n| n.trim().parse::<u64>().ok())
    };
    // Swap is absent on kernels built without it, which means none.
    let kib_total = kib("VmRSS:")?.saturating_add(kib("VmSwap:").unwrap_or(0));
    Some(kib_total.saturating_mul(1024))
}
