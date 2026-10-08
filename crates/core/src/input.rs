//! Real keyboard and mouse input for a running playtest. Roblox lets a plugin
//! neither synthesize input nor click a GUI, and ignores window messages
//! posted to an unfocused Studio, so the only way to press a key in the game
//! is the way a person does: bring Studio forward and send OS-level input.

pub enum Step {
    Keys { keys: Vec<String>, hold_ms: u64 },
    /// Viewport pixels, origin at the top-left corner of the 3D view.
    Click { x: i32, y: i32 },
    Wait(u64),
}

#[cfg(windows)]
mod win {
    use std::{mem::size_of, thread::sleep, time::Duration};

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT},
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::{
            Input::KeyboardAndMouse::{
                IsWindowEnabled, SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
                KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT,
            },
            WindowsAndMessaging::{
                BringWindowToTop, EnumChildWindows, EnumWindows, GetCursorPos, GetForegroundWindow,
                GetWindowTextW,
                GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetCursorPos,
                ShowWindow, SW_RESTORE,
                SetForegroundWindow, SetWindowPos, WindowFromPoint, HWND_NOTOPMOST, HWND_TOPMOST,
                SWP_NOMOVE, SWP_NOSIZE,
            },
        },
    };

    use super::Step;

    /// Scancode of a key by its position on a US keyboard, and whether it
    /// lives on the extended block. Roblox identifies keys by position, so
    /// `W` must be the key above `S` whatever the user's layout prints on it:
    /// going through the active layout would press `Z` on an AZERTY keyboard.
    fn scancode(name: &str) -> Result<(u16, bool), String> {
        const LETTERS: [u16; 26] = [
            0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31,
            0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
        ];

        let lower = name.trim().to_lowercase();
        let code = match lower.as_str() {
            "space" => (0x39, false),
            "enter" | "return" => (0x1C, false),
            "tab" => (0x0F, false),
            "escape" | "esc" => (0x01, false),
            "backspace" => (0x0E, false),
            "shift" => (0x2A, false),
            "ctrl" | "control" => (0x1D, false),
            "alt" => (ALT, false),
            "left" => (0x4B, true),
            "up" => (0x48, true),
            "right" => (0x4D, true),
            "down" => (0x50, true),
            "f11" => (0x57, false),
            "f12" => (0x58, false),
            _ => {
                let mut chars = lower.chars();
                match (chars.next(), chars.as_str()) {
                    (Some(c @ 'a'..='z'), "") => (LETTERS[(c as u8 - b'a') as usize], false),
                    (Some('0'), "") => (0x0B, false),
                    (Some(c @ '1'..='9'), "") => (0x02 + (c as u16 - '1' as u16), false),
                    (Some('f'), digits) => match digits.parse::<u16>() {
                        Ok(number @ 1..=10) => (0x3A + number, false),
                        _ => return Err(format!("Touche inconnue : {name}")),
                    },
                    _ => return Err(format!("Touche inconnue : {name}")),
                }
            }
        };
        Ok(code)
    }

    unsafe fn send_key(code: u16, extended: bool, up: bool) {
        let mut input: INPUT = std::mem::zeroed();
        input.r#type = INPUT_KEYBOARD;
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        input.Anonymous.ki = KEYBDINPUT {
            wVk: 0,
            wScan: code,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }

    unsafe fn send_mouse(flags: u32, dx: i32, dy: i32) {
        let mut input: INPUT = std::mem::zeroed();
        input.r#type = INPUT_MOUSE;
        input.Anonymous.mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }

    const ALT: u16 = 0x38;

    struct Search {
        size: (i32, i32),
        found: Option<RECT>,
    }

    unsafe extern "system" fn match_viewport(handle: HWND, param: LPARAM) -> i32 {
        let search = &mut *(param as *mut Search);
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if IsWindowVisible(handle) != 0 && GetWindowRect(handle, &mut rect) != 0 {
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            if (width - search.size.0).abs() <= 2 && (height - search.size.1).abs() <= 2 {
                search.found = Some(rect);
            }
        }
        1
    }

    /// The 3D view is the child window whose size equals the camera's
    /// viewport; nothing else in Studio exposes where it sits on screen.
    unsafe fn viewport_rect(studio: HWND, size: (i32, i32)) -> Result<RECT, String> {
        let mut search = Search { size, found: None };
        EnumChildWindows(studio, Some(match_viewport), &mut search as *mut _ as LPARAM);
        search
            .found
            .ok_or_else(|| "Viewport de jeu introuvable dans la fenêtre Studio".to_owned())
    }

    unsafe fn process_of(window: HWND) -> u32 {
        let mut process = 0;
        GetWindowThreadProcessId(window, &mut process);
        process
    }

    /// Studio counts as being in front when any of its windows is: during a
    /// test the foreground window is often a secondary one, not the main
    /// window the title search finds.
    unsafe fn in_front(studio: HWND) -> bool {
        let front = GetForegroundWindow();
        !front.is_null() && process_of(front) == process_of(studio)
    }

    /// Whether the pixel at this screen position belongs to Studio. Being the
    /// foreground process isn't enough: Studio can hold the foreground through
    /// a hidden helper window while another program covers its viewport, and
    /// a click would then land in that program.
    unsafe fn shows_at(studio: HWND, x: i32, y: i32) -> bool {
        let under = WindowFromPoint(POINT { x, y });
        !under.is_null() && process_of(under) == process_of(studio)
    }

    unsafe fn focus(studio: HWND, area: &RECT) -> Result<(), String> {
        // A modal dialog disables the main window: Studio would then swallow
        // every key and click, and the sequence would "succeed" for nothing.
        if IsWindowEnabled(studio) == 0 {
            return Err("Une boîte de dialogue est ouverte dans Roblox Studio et bloque les entrées : ferme-la d'abord".into());
        }
        let center = ((area.left + area.right) / 2, (area.top + area.bottom) / 2);
        if in_front(studio) && shows_at(studio, center.0, center.1) {
            return Ok(());
        }

        // Raising through the topmost band works even when Windows refuses
        // the foreground change, so the window is at least really on screen.
        SetWindowPos(studio, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        SetWindowPos(studio, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);

        // Windows only lets a process take the foreground if it shares the
        // input queue of the window that has it, or right after input. Both
        // are needed in practice: either one alone fails on some setups.
        let current = GetCurrentThreadId();
        let owner = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
        send_key(ALT, false, false);
        send_key(ALT, false, true);
        AttachThreadInput(current, owner, 1);
        BringWindowToTop(studio);
        SetForegroundWindow(studio);
        AttachThreadInput(current, owner, 0);
        sleep(Duration::from_millis(250));

        if !in_front(studio) || !shows_at(studio, center.0, center.1) {
            return Err("Impossible de mettre Roblox Studio au premier plan : aucune entrée n'a été envoyée au jeu".into());
        }
        Ok(())
    }

    unsafe fn play(studio: HWND, area: &RECT, steps: &[Step], resolved: &[Vec<(u16, bool)>]) -> Result<(), String> {
        focus(studio, area)?;

        for (step, keys) in steps.iter().zip(resolved) {
            // Input goes to whatever is in front: stop the moment that is
            // no longer Studio rather than type into another program.
            if !in_front(studio) {
                return Err("Roblox Studio a perdu le premier plan : séquence interrompue".into());
            }

            match step {
                Step::Keys { hold_ms, .. } => {
                    for (code, extended) in keys {
                        send_key(*code, *extended, false);
                    }
                    sleep(Duration::from_millis(*hold_ms));
                    for (code, extended) in keys.iter().rev() {
                        send_key(*code, *extended, true);
                    }
                }
                Step::Click { x, y } => {
                    let (screen_x, screen_y) = (area.left + x, area.top + y);
                    let inside = (area.left..area.right).contains(&screen_x)
                        && (area.top..area.bottom).contains(&screen_y);
                    if !inside || !shows_at(studio, screen_x, screen_y) {
                        return Err(format!(
                            "Le point ({x}, {y}) n'est pas dans le viewport visible de Studio : clic annulé"
                        ));
                    }
                    SetCursorPos(screen_x, screen_y);
                    // Roblox only registers a hover after a real move, and a
                    // button only activates if it was hovered for a few
                    // frames before the press.
                    send_mouse(MOUSEEVENTF_MOVE, 2, 0);
                    sleep(Duration::from_millis(40));
                    send_mouse(MOUSEEVENTF_MOVE, -2, 0);
                    sleep(Duration::from_millis(150));
                    send_mouse(MOUSEEVENTF_LEFTDOWN, 0, 0);
                    sleep(Duration::from_millis(90));
                    send_mouse(MOUSEEVENTF_LEFTUP, 0, 0);
                }
                Step::Wait(ms) => sleep(Duration::from_millis(*ms)),
            }
            sleep(Duration::from_millis(60));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::scancode;

        #[test]
        fn keys_are_named_by_their_position_on_a_us_keyboard() {
            // The four movement keys: on an AZERTY layout, going through the
            // active layout would press Z, Q, S, D instead.
            assert_eq!(scancode("W"), Ok((0x11, false)));
            assert_eq!(scancode("a"), Ok((0x1E, false)));
            assert_eq!(scancode("S"), Ok((0x1F, false)));
            assert_eq!(scancode("d"), Ok((0x20, false)));
            assert_eq!(scancode("Z"), Ok((0x2C, false)));
            assert_eq!(scancode("q"), Ok((0x10, false)));
            assert_eq!(scancode("m"), Ok((0x32, false)));
        }

        #[test]
        fn digits_function_keys_and_named_keys() {
            assert_eq!(scancode("1"), Ok((0x02, false)));
            assert_eq!(scancode("9"), Ok((0x0A, false)));
            assert_eq!(scancode("0"), Ok((0x0B, false)));
            assert_eq!(scancode("F1"), Ok((0x3B, false)));
            assert_eq!(scancode("f10"), Ok((0x44, false)));
            assert_eq!(scancode("F12"), Ok((0x58, false)));
            assert_eq!(scancode(" Space "), Ok((0x39, false)));
            assert_eq!(scancode("Left"), Ok((0x4B, true)));
            assert_eq!(scancode("alt"), Ok((0x38, false)));
        }

        #[test]
        fn an_unknown_key_is_an_error_not_a_guess() {
            for name in ["Banane", "", "F13", "F0", "é", "ab"] {
                assert!(scancode(name).is_err(), "{name}");
            }
        }
    }

    pub(crate) fn viewport_area(studio: HWND, size: (i32, i32)) -> Result<RECT, String> {
        unsafe { viewport_rect(studio, size) }
    }

    /// Whether a modal dialog is open in Studio, which shows as its main
    /// window being disabled.
    pub fn dialog_open(place_name: &str) -> Result<bool, String> {
        let studio = crate::screenshot::win::find(place_name)?;
        Ok(unsafe { IsWindowEnabled(studio) == 0 })
    }

    struct DialogSearch {
        process: u32,
        main: HWND,
        title: Option<String>,
    }

    unsafe extern "system" fn match_dialog(handle: HWND, param: LPARAM) -> i32 {
        let search = &mut *(param as *mut DialogSearch);
        if handle != search.main
            && IsWindowVisible(handle) != 0
            && IsWindowEnabled(handle) != 0
            && process_of(handle) == search.process
        {
            let mut buffer = [0u16; 256];
            let length = GetWindowTextW(handle, buffer.as_mut_ptr(), buffer.len() as i32);
            search.title = Some(String::from_utf16_lossy(&buffer[..length.max(0) as usize]));
            return 0;
        }
        1
    }

    /// The title of the dialog that blocks Studio, if one does. While it is
    /// open Studio ignores keys and clicks, and some of its own commands
    /// wait: the user has to answer it, the app can only say it is there.
    pub fn blocking_dialog(place_name: &str) -> Option<String> {
        let main = crate::screenshot::win::find(place_name).ok()?;
        unsafe {
            if IsWindowEnabled(main) != 0 {
                return None;
            }
            let mut search = DialogSearch { process: process_of(main), main, title: None };
            EnumWindows(Some(match_dialog), &mut search as *mut _ as LPARAM);
            Some(search.title.unwrap_or_default())
        }
    }

    pub fn run(place_name: &str, viewport: (i32, i32), steps: &[Step]) -> Result<(), String> {
        let studio = crate::screenshot::win::find(place_name)?;

        // Resolved up front so that a typo can't leave a key held down.
        let mut resolved = Vec::new();
        for step in steps {
            if let Step::Keys { keys, .. } = step {
                resolved.push(
                    keys.iter()
                        .map(|key| scancode(key))
                        .collect::<Result<Vec<_>, _>>()?,
                );
            } else {
                resolved.push(Vec::new());
            }
        }

        unsafe {
            // A minimized window has no layout: its 3D view can't be located,
            // and it can't take the foreground either.
            if IsIconic(studio) != 0 {
                ShowWindow(studio, SW_RESTORE);
                sleep(Duration::from_millis(700));
            }
            let area = viewport_rect(studio, viewport)?;
            let previous = GetForegroundWindow();
            let mut cursor = POINT { x: 0, y: 0 };
            GetCursorPos(&mut cursor);

            let outcome = play(studio, &area, steps, &resolved);

            // Hand the desktop back the way it was found, whether or not the
            // sequence went through.
            SetCursorPos(cursor.x, cursor.y);
            if !previous.is_null() && process_of(previous) != process_of(studio) {
                SetForegroundWindow(previous);
            }
            outcome
        }
    }
}

#[cfg(windows)]
pub(crate) use win::viewport_area;
#[cfg(windows)]
pub use win::{blocking_dialog, dialog_open, run};

#[cfg(not(windows))]
pub fn dialog_open(_place_name: &str) -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(windows))]
pub fn blocking_dialog(_place_name: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::describe_dialog;

    #[test]
    fn an_untitled_studio_dialog_is_named_by_its_likeliest_cause() {
        for title in ["", "RobloxStudio", "Roblox Studio"] {
            assert!(describe_dialog(title).contains("Auto Recovery"), "{title}");
        }
        assert_eq!(describe_dialog("Publish Experience"), "la boîte de dialogue « Publish Experience »");
    }
}

/// How to name that dialog to the user. Studio gives several of its dialogs
/// no title of their own, the crash-recovery one among them.
pub fn describe_dialog(title: &str) -> String {
    if title.is_empty() || title == "RobloxStudio" || title == "Roblox Studio" {
        "une boîte de dialogue (souvent « Auto Recovery », après une fermeture brutale de Studio)".to_owned()
    } else {
        format!("la boîte de dialogue « {title} »")
    }
}

#[cfg(not(windows))]
pub fn run(_place_name: &str, _viewport: (i32, i32), _steps: &[Step]) -> Result<(), String> {
    Err("Les entrées clavier et souris ne sont disponibles que sous Windows".into())
}
