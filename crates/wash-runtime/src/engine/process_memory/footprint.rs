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
        use sysinfo::{Pid, RefreshKind, System};

        let pid = Pid::from_u32(pid);
        let mut system = System::new_with_specifics(RefreshKind::new());
        sysinfo_footprint(&mut system, pid)
    }
}

/// Reads the footprint over and over without paying to set up each reading.
///
/// On Linux it keeps `/proc/self/status` open and re-reads it from the start
/// into one buffer: no open, and so no `EMFILE` on a host at its descriptor
/// limit, and no allocation per sample.
///
/// Elsewhere `sysinfo` keeps one `System` for the life of the reader rather
/// than building one per sample. Its resident size (the working set on
/// Windows) does not include page tables, so there the reading is low by
/// what Linux now counts in `VmPTE`, about a tenth of a dense warm pool.
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
        let pid = sysinfo::get_current_pid().ok()?;
        sysinfo_footprint(&mut self.system, pid)
    }
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "linux"))))]
#[cfg_attr(
    all(coverage_nightly, any(target_os = "macos", target_os = "linux")),
    coverage(off)
)]
fn sysinfo_footprint(system: &mut sysinfo::System, pid: sysinfo::Pid) -> Option<u64> {
    // A failed refresh can leave the previous process entry in the cache.
    let updated = system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        false,
        sysinfo::ProcessRefreshKind::new().with_memory(),
    );
    if updated == 0 {
        return None;
    }
    system.process(pid).map(|process| process.memory())
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

/// `RssAnon + VmSwap + VmPTE + VmPMD` out of `/proc/self/status`, in bytes,
/// with `VmRSS` in place of `RssAnon` on a kernel too old (before 4.5) to
/// report it.
///
/// `VmPTE` and `VmPMD` are the page tables mapping this process, memory the
/// kernel allocates for it and charges to it (the OOM killer counts it). From
/// Linux 4.15 `VmPTE` covers every level and `VmPMD` is gone; on 4.0 to 4.14
/// `VmPTE` is the last level only and `VmPMD` the level above it, so both are
/// read. A dense pool makes them large: every slot is its own sparse region,
/// so each warm instance needs page-table pages of its own for its memory,
/// its image and its stack, about 9 KiB an instance at a 64 MiB layout,
/// 13.7 GiB at 1.5M warm instances. Left out, a host that sized
/// `max-process-memory` to its machine is OOM-killed with its guard still
/// reading under the limit.
///
/// Page tables outlive the instance that needed them. Wasmtime decommits a
/// pooled slot with `madvise(MADV_DONTNEED)`, which returns its pages but, on
/// most kernels, keeps the page tables that mapped them, so they stay counted
/// until the pool is unmapped. That is memory the host still holds, like its
/// allocator's free lists, and the guard treats it the same way (see "When
/// refusals stop" in the module docs).
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
    let mut anon = None;
    let mut rss = None;
    let mut swap = None;
    let mut pte = None;
    let mut pmd = None;
    for line in status.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let field = match name {
            "RssAnon" => &mut anon,
            "VmRSS" => &mut rss,
            "VmSwap" => &mut swap,
            "VmPTE" => &mut pte,
            "VmPMD" => &mut pmd,
            _ => continue,
        };
        field.get_or_insert(value);
    }

    // Absence is distinct from corruption: optional fields may be absent,
    // but a present field must parse or the entire sample is unavailable.
    let kib = |value: &str| -> Option<u64> { value.trim().strip_suffix("kB")?.trim().parse().ok() };
    let resident = kib(anon.or(rss)?)?;
    // VmPTE is mandatory on supported Linux kernels. Missing or malformed
    // accounting must not silently restore the page-table undercount.
    let page_tables = kib(pte?)?;
    // Swap may be absent without CONFIG_SWAP; modern kernels omit VmPMD.
    let kib_total = resident
        .saturating_add(swap.map_or(Some(0), kib)?)
        .saturating_add(page_tables)
        .saturating_add(pmd.map_or(Some(0), kib)?);
    Some(kib_total.saturating_mul(1024))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use std::io::{BufRead as _, Write as _};

    #[test]
    fn a_failed_refresh_does_not_republish_cached_memory() -> anyhow::Result<()> {
        const CHILD: &str = "WASH_MEMORY_READER_CHILD";
        const READY: &str = "memory reader ready";
        if std::env::var_os(CHILD).is_some() {
            println!("{READY}");
            std::io::stdout().flush()?;
            std::io::copy(&mut std::io::stdin(), &mut std::io::sink())?;
            return Ok(());
        }

        let (_, module) = module_path!().split_once("::").context("no test module")?;
        let test = format!("{module}::a_failed_refresh_does_not_republish_cached_memory");
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", &test, "--nocapture"])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let output = child.stdout.take().context("missing child stdout")?;
        let mut lines = std::io::BufReader::new(output).lines();
        let mut ready = false;
        for line in lines.by_ref() {
            if line?.ends_with(READY) {
                ready = true;
                break;
            }
        }
        let pid = sysinfo::Pid::from_u32(child.id());
        let mut system = sysinfo::System::new_with_specifics(sysinfo::RefreshKind::new());
        let initial = sysinfo_footprint(&mut system, pid);
        drop(child.stdin.take());
        // Keep stdout open until the child harness writes its test summary.
        let status = child.wait()?;
        drop(lines);

        assert!(status.success());
        assert!(ready, "the child reached its wait for input");
        assert!(initial.is_some(), "the live child has readable memory");
        assert!(
            system.process(pid).is_some(),
            "the previous sample is cached"
        );
        assert_eq!(sysinfo_footprint(&mut system, pid), None);
        Ok(())
    }
}
