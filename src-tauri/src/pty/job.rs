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

/// Orphan checks for PLAN.md phase 5: closing a job, or the app dying while it
/// holds one, must take the whole tree with it.
#[cfg(all(test, windows))]
mod tests {
    use super::Job;
    use std::io::{BufRead, BufReader};
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command, Stdio};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

    const CHILD_ENV: &str = "SPECTACLI_JOB_CRASH_CHILD";

    /// A long-running process that ignores stdin.
    fn sleeper() -> Child {
        Command::new("ping")
            .args(["-n", "120", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .expect("start ping")
    }

    /// True if process `pid` exits within `ms`.
    fn exits_within(pid: u32, ms: u32) -> bool {
        // SAFETY: plain Win32 calls on a handle we open and close here.
        unsafe {
            let h: HANDLE = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if h.is_null() {
                return true; // already gone
            }
            let done = WaitForSingleObject(h, ms) == WAIT_OBJECT_0;
            CloseHandle(h);
            done
        }
    }

    #[test]
    fn dropping_job_kills_tree() {
        let job = Job::new().unwrap();
        let mut child = sleeper();
        job.assign(child.as_raw_handle()).unwrap();
        assert!(job.pids().unwrap().contains(&child.id()));
        drop(job);
        assert!(exits_within(child.id(), 5000), "child outlived its job");
        let _ = child.wait();
    }

    /// Re-runs this test binary as a stand-in for Spectacli: it puts a
    /// grandchild in a job, then gets killed like a crash would. Windows
    /// closes its job handle, which must end the grandchild.
    #[test]
    fn crash_of_job_owner_kills_tree() {
        if std::env::var_os(CHILD_ENV).is_some() {
            let job = Job::new().unwrap();
            let grandchild = sleeper();
            job.assign(grandchild.as_raw_handle()).unwrap();
            // libtest prints its status on the same line, so the parent
            // looks for the marker anywhere.
            println!("GRANDCHILD {}", grandchild.id());
            std::thread::sleep(std::time::Duration::from_secs(120));
            unreachable!("the parent kills this process");
        }

        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pty::job::tests::crash_of_job_owner_kills_tree", "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = BufReader::new(owner.stdout.take().unwrap())
            .lines()
            .map_while(Result::ok)
            .find_map(|l| l.split_once("GRANDCHILD ").and_then(|(_, p)| p.trim().parse::<u32>().ok()))
            .expect("child reports its grandchild");
        // Hold a handle so the PID cannot be reused before we check it.
        // SAFETY: plain Win32 calls; closed below.
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!h.is_null(), "grandchild should be running");

        owner.kill().unwrap(); // TerminateProcess: no destructors run
        owner.wait().unwrap();

        // SAFETY: valid handle from above.
        let gone = unsafe { WaitForSingleObject(h, 5000) } == WAIT_OBJECT_0;
        unsafe { CloseHandle(h) };
        assert!(gone, "grandchild {pid} was orphaned after its job owner died");
    }
}
