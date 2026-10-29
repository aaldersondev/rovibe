//! Keeps Windows from going to sleep while an agent works: an agent left on a
//! long task would otherwise stop with the PC. Only sleep is held off; the
//! screen still turns off as the user set it.

use std::{sync::atomic::Ordering, time::Duration};

use crate::state::Shared;

/// Whether any agent is in the middle of a turn. One waiting for the user,
/// or done, doesn't need the PC.
fn busy(state: &Shared) -> bool {
    state.sessions.lock().unwrap().values().any(|session| {
        !session.exited.load(Ordering::Relaxed) && matches!(session.status.lock().unwrap().state, "starting" | "working")
    })
}

/// Windows ties the request to the thread that makes it, so one thread of
/// its own holds it for as long as the server runs.
pub fn watch(state: &Shared) {
    let state = state.clone();
    let _ = std::thread::Builder::new().name("awake".into()).spawn(move || {
        let mut held = false;
        loop {
            let wanted = busy(&state);
            if wanted != held {
                hold(wanted);
                held = wanted;
                crate::log::info(if wanted {
                    "Mise en veille de Windows suspendue : un agent travaille"
                } else {
                    "Mise en veille de Windows de nouveau permise"
                });
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(windows)]
fn hold(awake: bool) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    let flags = if awake { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
    unsafe { SetThreadExecutionState(flags) };
}

#[cfg(not(windows))]
fn hold(_awake: bool) {}
