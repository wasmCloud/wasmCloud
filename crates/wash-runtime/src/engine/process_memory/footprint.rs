//! Platform memory acquisition, separate from guard policy and sampling.

/// This process's memory footprint, in bytes, counting pages the kernel has
/// swapped or compressed. `None` where it cannot be read.
pub fn process_footprint() -> Option<u64> {
    FootprintReader::new().read()
}

/// Another process's footprint, counted the same way as
/// [`process_footprint`], so an outside observer such as a benchmark compares
/// like with like against the host's own guard. `None` where it cannot be
/// read, including once the process has gone.
pub fn pid_footprint(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux_footprint(&std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?)
    }
    #[cfg(target_os = "macos")]
    {
        read_footprint(pid)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

        let pid = Pid::from_u32(pid);
        let mut system = System::new_with_specifics(RefreshKind::new());
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            false,
            ProcessRefreshKind::new().with_memory(),
        );
        system.process(pid).map(|process| process.memory())
    }
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
        read_footprint(std::process::id())
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
fn read_footprint(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: `RUSAGE_INFO_V2` tells the kernel to write one
    // `rusage_info_v2`, which is exactly what the buffer holds. A pid that is
    // gone or not ours to read returns an error instead.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, info.as_mut_ptr().cast()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: a zero return means the kernel filled the buffer, and it was
    // zeroed before that, so every field is initialised either way.
    Some(unsafe { info.assume_init() }.ri_phys_footprint)
}

/// `RssAnon + VmSwap` out of `/proc/self/status`, in bytes, or
/// `VmRSS + VmSwap` on a kernel too old (before 4.5) to report `RssAnon`.
///
/// `VmRSS` counts a shared page once for every mapping that has it faulted
/// in, and wasmtime maps each instance's copy-on-write initial-memory image
/// separately, so a pool of warm instances of one snapshotted component would
/// read as one copy of its image per instance though memory holds one. Image
/// pages a guest writes become anonymous, so they are still counted. Shmem
/// (`RssShmem`) is left out along with file pages: wasmtime's images are
/// memfds, which the kernel reports as shmem, so counting it would bring the
/// same per-mapping overcount back.
#[cfg(any(target_os = "linux", test))]
pub(super) fn linux_footprint(status: &str) -> Option<u64> {
    let kib = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|n| n.trim().parse::<u64>().ok())
    };
    let resident = kib("RssAnon:").or_else(|| kib("VmRSS:"))?;
    // Swap is absent on kernels built without it, which means none.
    let kib_total = resident.saturating_add(kib("VmSwap:").unwrap_or(0));
    Some(kib_total.saturating_mul(1024))
}
