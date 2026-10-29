//! The PATH the app was started with isn't always the user's. A program
//! installed since stays out of it until Windows is restarted, and Claude
//! Code's own installer often leaves its folder out of the PATH altogether.
//! The server's PATH is completed with the one Windows holds now and with
//! the folders the agents usually install to: they are then found, and so
//! is everything they need in what the server starts.

use std::path::{Path, PathBuf};

/// Adds to the server's PATH the folders it lacks. Cheap enough to run each
/// time the programs are looked up again.
pub fn complete_path() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut entries: Vec<PathBuf> = std::env::split_paths(&current).collect();
    let known = entries.len();
    let wanted = system_path().into_iter().flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>());
    for folder in wanted.chain(usual_folders()) {
        if !folder.as_os_str().is_empty() && !entries.iter().any(|entry| same(entry, &folder)) && folder.is_dir() {
            entries.push(folder);
        }
    }
    if entries.len() > known {
        if let Ok(path) = std::env::join_paths(&entries) {
            std::env::set_var("PATH", path);
        }
    }
}

fn same(a: &Path, b: &Path) -> bool {
    let trim = |path: &Path| path.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    trim(a) == trim(b)
}

/// Where Claude Code, Codex, Node.js and Git go when installed the usual
/// ways: Claude Code's installer, npm, winget, and the Git and Node.js
/// installers.
fn usual_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(home) = dirs::home_dir() {
        folders.push(home.join(".local").join("bin"));
    }
    if let Some(roaming) = dirs::data_dir() {
        folders.push(roaming.join("npm"));
    }
    if let Some(local) = dirs::data_local_dir() {
        let winget = local.join("Microsoft").join("WinGet");
        folders.push(winget.join("Links"));
        let packages = std::fs::read_dir(winget.join("Packages")).into_iter().flatten().flatten();
        folders.extend(
            packages
                .map(|package| package.path())
                .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("Anthropic.ClaudeCode_"))),
        );
        folders.push(local.join("Programs").join("Git").join("cmd"));
    }
    if let Some(programs) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        folders.push(programs.join("Git").join("cmd"));
        folders.push(programs.join("nodejs"));
    }
    folders
}

/// The PATH Windows gives a program started now, the machine's then the
/// user's, as the registry holds them.
#[cfg(windows)]
fn system_path() -> Vec<std::ffi::OsString> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let read = |root: HKEY, key: &str| {
        let (key, name) = (wide(key), wide("Path"));
        // An environment variable holds 32767 characters at most, and
        // asking for a plain string has Windows expand the %VARIABLES%.
        let mut buffer = vec![0u16; 32768];
        let mut size = (buffer.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (status == 0).then(|| {
            let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
            std::ffi::OsString::from_wide(&buffer[..length])
        })
    };
    [
        read(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        read(HKEY_CURRENT_USER, "Environment"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(not(windows))]
fn system_path() -> Vec<std::ffi::OsString> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_match_whatever_their_case_and_last_slash() {
        assert!(same(Path::new(r"C:\Users\A\.local\bin\"), Path::new(r"c:\users\a\.local\BIN")));
        assert!(!same(Path::new(r"C:\Users\A\.local\bin"), Path::new(r"C:\Users\A\.local")));
    }

    #[cfg(windows)]
    #[test]
    fn reads_the_path_windows_holds() {
        let path = system_path();
        assert!(!path.is_empty());
        assert!(path.iter().any(|path| path.to_string_lossy().to_lowercase().contains("system32")));
    }
}
