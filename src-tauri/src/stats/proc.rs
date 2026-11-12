//! Per-process reads for the sampler: memory counters, CPU times, and the
//! app's own process tree.

use std::collections::HashSet;

#[derive(Clone, Copy, Debug)]
pub struct Proc {
    pub pid: u32,
    pub private_bytes: u64,
    pub working_set: u64,
    /// User + kernel time, 100 ns units.
    pub cpu: u64,
    /// Creation time (FILETIME), to tell a reused PID from the old process.
    pub created: u64,
}

#[cfg(windows)]
mod imp {
    use super::Proc;
    use std::collections::{HashMap, HashSet};
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::{
        GetActiveProcessorCount, GetProcessTimes, OpenProcess, ALL_PROCESSOR_GROUPS,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: we own the handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn ft(t: FILETIME) -> u64 {
        (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64
    }

    pub fn cores() -> u32 {
        // SAFETY: no pointers involved. Counts every processor group, since
        // job and process CPU time can come from any of them.
        unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1)
    }

    pub fn read(pid: u32) -> Option<Proc> {
        // SAFETY: plain Win32 calls on a handle we own; out-structs are
        // plain data with their size set.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
            let h = if h.is_null() { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) } else { h };
            if h.is_null() {
                return None;
            }
            let h = Handle(h);
            let mut mem: PROCESS_MEMORY_COUNTERS_EX = zeroed();
            mem.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            if K32GetProcessMemoryInfo(h.0, &mut mem as *mut _ as *mut _, mem.cb) == 0 {
                return None;
            }
            let (mut created, mut exited, mut kernel, mut user) = (zeroed(), zeroed(), zeroed(), zeroed());
            let times = GetProcessTimes(h.0, &mut created, &mut exited, &mut kernel, &mut user) != 0;
            Some(Proc {
                pid,
                private_bytes: mem.PrivateUsage as u64,
                working_set: mem.WorkingSetSize as u64,
                cpu: if times { ft(kernel) + ft(user) } else { 0 },
                created: if times { ft(created) } else { 0 },
            })
        }
    }

    /// (pid, parent pid) of every process on the system.
    fn snapshot() -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        // SAFETY: the snapshot handle is owned and closed; the entry has its
        // size set as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let snap = Handle(snap);
            let mut e: PROCESSENTRY32W = zeroed();
            e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap.0, &mut e) != 0;
            while ok {
                out.push((e.th32ProcessID, e.th32ParentProcessID));
                ok = Process32NextW(snap.0, &mut e) != 0;
            }
        }
        out
    }

    /// This process and its descendants, not descending into `exclude`
    /// (the agent trees). A child must have started after its parent, so a
    /// PID left behind by a dead parent and then reused is not adopted.
    pub fn own_tree(exclude: &HashSet<u32>) -> Vec<Proc> {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, parent) in snapshot() {
            if pid != parent {
                children.entry(parent).or_default().push(pid);
            }
        }
        let Some(me) = read(std::process::id()) else { return Vec::new() };
        let mut out = vec![me];
        let mut i = 0;
        while i < out.len() {
            let parent = out[i];
            for &pid in children.get(&parent.pid).map(Vec::as_slice).unwrap_or_default() {
                if exclude.contains(&pid) {
                    continue;
                }
                if let Some(p) = read(pid).filter(|p| p.created >= parent.created) {
                    out.push(p);
                }
            }
            i += 1;
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Proc;
    use std::collections::HashSet;
    pub fn cores() -> u32 {
        std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
    }
    pub fn read(_pid: u32) -> Option<Proc> {
        None
    }
    pub fn own_tree(_exclude: &HashSet<u32>) -> Vec<Proc> {
        Vec::new()
    }
}

pub fn cores() -> u32 {
    imp::cores()
}

pub fn read(pid: u32) -> Option<Proc> {
    imp::read(pid)
}

pub fn own_tree(exclude: &HashSet<u32>) -> Vec<Proc> {
    imp::own_tree(exclude)
}
