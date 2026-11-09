//! Process memory counters (Windows only), for enumeration and index benchmarks.

use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Working-set sizes of the current process, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMemory {
    /// Current working set.
    pub working_set: u64,
    /// Peak working set since the process started.
    pub peak_working_set: u64,
    /// Current private (commit) bytes.
    pub pagefile_usage: u64,
}

/// Reads the current process's memory counters (`GetProcessMemoryInfo`). `None` if the call
/// fails, which it does not in practice.
#[must_use]
pub fn process_memory() -> Option<ProcessMemory> {
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing; `counters` is a
    // live, writable struct whose size is passed as `cb`.
    let ok = unsafe {
        GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, counters.cb).is_ok()
    };
    ok.then_some(ProcessMemory {
        working_set: counters.WorkingSetSize as u64,
        peak_working_set: counters.PeakWorkingSetSize as u64,
        pagefile_usage: counters.PagefileUsage as u64,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_nonzero_working_set() {
        let m = super::process_memory().expect("GetProcessMemoryInfo");
        assert!(m.working_set > 0);
        assert!(m.peak_working_set >= m.working_set);
    }
}
