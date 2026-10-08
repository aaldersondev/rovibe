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
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
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

    pub fn adopt(process_id: u32) {
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id);
            if !process.is_null() {
                AssignProcessToJobObject(job() as _, process);
                CloseHandle(process);
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

#[cfg(not(windows))]
pub fn adopt(_process_id: u32) {}
