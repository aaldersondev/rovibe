//! Ties the processes the app starts to the app's own lifetime.
//!
//! Dropping a child handle only kills it on an orderly shutdown. When the app
//! is killed, or exits abruptly to let an update install, the sync servers
//! and agents would survive it: still holding their ports, and still locking
//! the very files the installer has to replace.

#[cfg(windows)]
mod win {
    use std::sync::OnceLock;

    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
        },
    };

    /// The job's only handle lives as long as this process. Windows closes it
    /// when the process ends, however it ends, and that kills what is in it.
    /// The app itself stays out of the job, so an installer it launches is
    /// free to outlive it.
    fn job() -> isize {
        static JOB: OnceLock<isize> = OnceLock::new();
        *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            job as isize
        })
    }

    fn assign(job: isize, process_id: u32) {
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id);
            if !process.is_null() {
                AssignProcessToJobObject(job as _, process);
                CloseHandle(process);
            }
        }
    }

    pub fn adopt(process_id: u32) {
        assign(job(), process_id);
    }

    /// A job of its own for one session, nested in the app's: ending it ends
    /// the agent and every process the agent started, and nothing else.
    pub struct Family(isize);

    impl Family {
        pub fn new() -> Self {
            Self(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) } as isize)
        }

        pub fn adopt(&self, process_id: u32) {
            assign(self.0, process_id);
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0 as _, 1);
            }
        }
    }

    impl Drop for Family {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0 as _);
            }
        }
    }
}

/// Makes a process the app started die with the app. Processes it starts in
/// turn are covered too.
#[cfg(windows)]
pub fn adopt(process_id: u32) {
    win::adopt(process_id);
}

#[cfg(windows)]
pub use win::Family;

#[cfg(not(windows))]
pub fn adopt(_process_id: u32) {}

#[cfg(not(windows))]
pub struct Family;

#[cfg(not(windows))]
impl Family {
    pub fn new() -> Self {
        Self
    }
    pub fn adopt(&self, _process_id: u32) {}
    pub fn terminate(&self) {}
}
