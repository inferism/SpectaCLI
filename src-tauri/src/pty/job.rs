//! One Windows Job Object per agent (PLAN.md 2.3). Closing the handle, on
//! drop or when the app dies, kills the whole process tree.

#[cfg(windows)]
mod imp {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::RawHandle;
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);

    // The handle is only used through thread-safe kernel calls.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> io::Result<Self> {
            // SAFETY: plain Win32 calls; the handle is owned by `Job` from here on.
            unsafe {
                let h = CreateJobObjectW(null(), null());
                if h.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = Job(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub fn assign(&self, process: RawHandle) -> io::Result<()> {
            // SAFETY: `process` is a live process handle owned by the caller.
            if unsafe { AssignProcessToJobObject(self.0, process as HANDLE) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) -> io::Result<()> {
            // SAFETY: valid job handle.
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// User + kernel CPU time of every process that has ever been in the
        /// job, exited ones included, in 100 ns units.
        pub fn cpu_time(&self) -> io::Result<u64> {
            // SAFETY: the struct is plain data sized for this info class.
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = zeroed();
                if QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok((info.TotalUserTime + info.TotalKernelTime).max(0) as u64)
            }
        }

        /// Every live PID in the tree.
        pub fn pids(&self) -> io::Result<Vec<u32>> {
            const MAX: usize = 1024;
            #[repr(C)]
            struct List {
                head: JOBOBJECT_BASIC_PROCESS_ID_LIST,
                rest: [usize; MAX - 1],
            }
            // SAFETY: `List` is a header followed by room for MAX ids, as the
            // API expects; zeroed is a valid bit pattern.
            unsafe {
                let mut list: List = zeroed();
                if QueryInformationJobObject(
                    self.0,
                    JobObjectBasicProcessIdList,
                    &mut list as *mut _ as *mut _,
                    size_of::<List>() as u32,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let n = list.head.NumberOfProcessIdsInList as usize;
                let ids = std::slice::from_raw_parts(list.head.ProcessIdList.as_ptr(), n.min(MAX));
                Ok(ids.iter().map(|&p| p as u32).collect())
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: we own the handle. KILL_ON_JOB_CLOSE ends the tree.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Non-Windows stub so the crate builds and tests elsewhere; the PTY's own
/// kill is the only cleanup there.
#[cfg(not(windows))]
mod imp {
    use std::io;
    pub struct Job;
    impl Job {
        pub fn new() -> io::Result<Self> {
            Ok(Job)
        }
        pub fn terminate(&self) -> io::Result<()> {
            Ok(())
        }
        pub fn pids(&self) -> io::Result<Vec<u32>> {
            Ok(Vec::new())
        }
        pub fn cpu_time(&self) -> io::Result<u64> {
            Ok(0)
        }
    }
}

pub use imp::Job;
