//! Effective memory available to a benchmark and its child processes.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// Physical RAM capped by readable cgroup v1/v2 limits. Benchmarks reserve
/// half of this budget for their host and leave the rest for the load driver,
/// local registry, NATS, and other processes in the same container.
pub fn memory_budget(physical: u64) -> Option<u64> {
    memory_budget_with(physical, |path| std::fs::read_to_string(path).ok())
}

fn memory_budget_with(physical: u64, read: impl Fn(&Path) -> Option<String>) -> Option<u64> {
    let membership = read(Path::new("/proc/self/cgroup")).unwrap_or_default();
    let mut paths = BTreeSet::from([
        PathBuf::from("/sys/fs/cgroup/memory.max"),
        PathBuf::from("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
    ]);
    for line in membership.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(_), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (root, file) = if controllers.is_empty() {
            (Path::new("/sys/fs/cgroup"), "memory.max")
        } else if controllers.split(',').any(|c| c == "memory") {
            (Path::new("/sys/fs/cgroup/memory"), "memory.limit_in_bytes")
        } else {
            continue;
        };
        // Namespace-relative membership can contain '..'. The mount-root
        // limit above still applies; do not read outside that mount.
        let relative = Path::new(path.trim_start_matches('/'));
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let mut directory = root.join(relative);
        while directory.starts_with(root) {
            paths.insert(directory.join(file));
            if directory == root || !directory.pop() {
                break;
            }
        }
    }
    let cgroups = paths
        .iter()
        .filter_map(|path| read(path))
        .filter_map(|value| value.trim().parse::<u64>().ok().filter(|n| *n < (1 << 60)));
    Some(physical)
        .filter(|n| *n > 0)
        .into_iter()
        .chain(cgroups)
        .min()
        .filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_budget_overrides_physical_ram_and_checks_ancestors() {
        let gib = 1 << 30;
        let read = |path: &Path| match path.to_str()? {
            "/proc/self/cgroup" => Some("0::/tenant/worker".to_string()),
            "/sys/fs/cgroup/memory.max" => Some("max".to_string()),
            "/sys/fs/cgroup/tenant/memory.max" => Some((gib / 2).to_string()),
            "/sys/fs/cgroup/tenant/worker/memory.max" => Some(gib.to_string()),
            _ => None,
        };
        assert_eq!(memory_budget_with(64 * gib, read), Some(gib / 2));
        assert_eq!(memory_budget_with(0, read), Some(gib / 2));
        assert_eq!(memory_budget_with(gib / 4, read), Some(gib / 4));
    }

    #[test]
    fn legacy_and_unlimited_cgroups_have_explicit_fallbacks() {
        assert_eq!(memory_budget_with(4096, |_| None), Some(4096));
        assert_eq!(memory_budget_with(0, |_| None), None);
        let legacy = |path: &Path| match path.to_str()? {
            "/sys/fs/cgroup/memory/memory.limit_in_bytes" => Some("1024".into()),
            _ => None,
        };
        assert_eq!(memory_budget_with(4096, legacy), Some(1024));
        assert_eq!(
            memory_budget_with(4096, |_| Some("9223372036854771712".into())),
            Some(4096)
        );
        assert_eq!(memory_budget_with(4096, |_| Some("0".into())), None);
    }
}
