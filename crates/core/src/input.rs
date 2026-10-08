//! Keyboard and mouse input for a running playtest. Roblox lets a plugin
//! neither synthesize input nor click a GUI, so it comes from outside.
//!
//! The game reads the messages of one window, the 3D view, whether or not
//! Studio is in front: keys and clicks posted to that window reach the game
//! while the user keeps the foreground, the keyboard and the mouse. Posted to
//! Studio's main window they are lost, which long made this look impossible.
//!
//! Studio's own shortcuts (publishing) are read by its main window, which
//! looks up Shift, Ctrl and Alt in its keyboard state rather than in the
//! message. They too are pressed from behind: that state is set for the time
//! of the key, then put back. Going through the foreground, the way a person
//! presses them, remains the fallback.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn named(name: Option<&str>) -> Result<Self, String> {
        match name.map(str::to_lowercase).as_deref() {
            None | Some("left" | "gauche") => Ok(Self::Left),
            Some("right" | "droit") => Ok(Self::Right),
            Some("middle" | "milieu") => Ok(Self::Middle),
            Some(other) => Err(format!("Bouton inconnu : {other} (left, right ou middle)")),
        }
    }
}

/// Positions are viewport pixels, origin at the top-left corner of the 3D view.
pub enum Step {
    Keys { keys: Vec<String>, hold_ms: u64 },
    Click { x: i32, y: i32, button: Button },
    /// Moves the pointer there and leaves it, for hover effects.
    Move { x: i32, y: i32 },
    /// Presses at `from`, travels to `to` over `ms`, releases.
    Drag { from: (i32, i32), to: (i32, i32), button: Button, ms: u64 },
    /// Wheel notches at a point; positive is away from the user.
    Scroll { x: i32, y: i32, notches: i32 },
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
                GetKeyboardState, IsWindowEnabled, SendInput, SetKeyboardState, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
                KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT,
            },
            WindowsAndMessaging::{
                BringWindowToTop, EnumChildWindows, EnumWindows, GetCursorPos, GetForegroundWindow,
                GetWindowTextW,
                GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, PeekMessageW,
                PostMessageW, MSG, PM_NOREMOVE, WM_ACTIVATE,
                SetCursorPos, ShowWindow, SW_RESTORE, SW_SHOWNOACTIVATE, WM_KEYDOWN, WM_KEYUP,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
                WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
                SetForegroundWindow, SetWindowPos, WindowFromPoint, HWND_NOTOPMOST, HWND_TOPMOST,
                SWP_NOMOVE, SWP_NOSIZE,
            },
        },
    };

    use super::{Button, Step};

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

    /// The virtual-key code that goes with `scancode` on a US keyboard. A
    /// posted key message carries both, and they have to agree.
    fn virtual_key(name: &str) -> Result<u16, String> {
        let lower = name.trim().to_lowercase();
        Ok(match lower.as_str() {
            "space" => 0x20,
            "enter" | "return" => 0x0D,
            "tab" => 0x09,
            "escape" | "esc" => 0x1B,
            "backspace" => 0x08,
            "shift" => 0x10,
            "ctrl" | "control" => 0x11,
            "alt" => VK_ALT,
            "left" => 0x25,
            "up" => 0x26,
            "right" => 0x27,
            "down" => 0x28,
            _ => {
                let mut chars = lower.chars();
                match (chars.next(), chars.as_str()) {
                    (Some(c @ ('a'..='z' | '0'..='9')), "") => c.to_ascii_uppercase() as u16,
                    (Some('f'), digits) => match digits.parse::<u16>() {
                        Ok(number @ 1..=12) => 0x6F + number,
                        _ => return Err(format!("Touche inconnue : {name}")),
                    },
                    _ => return Err(format!("Touche inconnue : {name}")),
                }
            }
        })
    }

    const VK_ALT: u16 = 0x12;

    struct Key {
        virtual_key: u16,
        scan: u16,
        extended: bool,
    }

    /// Posts one key transition to a window, as the keyboard driver words it:
    /// repeat count, scancode, extended flag, and for a release the two bits
    /// that say the key was down and is going up.
    unsafe fn post_key(view: HWND, key: &Key, up: bool) {
        let mut detail = 1u32 | (u32::from(key.scan) << 16);
        if key.extended {
            detail |= 1 << 24;
        }
        if up {
            detail |= 0xC000_0000;
        }
        let alt = key.virtual_key == VK_ALT;
        if alt && !up {
            detail |= 1 << 29;
        }
        let message = match (alt, up) {
            (false, false) => WM_KEYDOWN,
            (false, true) => WM_KEYUP,
            (true, false) => WM_SYSKEYDOWN,
            (true, true) => WM_SYSKEYUP,
        };
        PostMessageW(view, message, key.virtual_key as usize, detail as isize);
    }

    unsafe fn post_mouse(view: HWND, message: u32, buttons: usize, x: i32, y: i32) {
        let position = ((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF);
        PostMessageW(view, message, buttons, position as isize);
    }

    /// The messages of a mouse button, and the flag that says it is held
    /// while other mouse messages go by.
    fn button_messages(button: Button) -> (u32, u32, usize) {
        match button {
            Button::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP, 0x01),
            Button::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP, 0x02),
            Button::Middle => (WM_MBUTTONDOWN, WM_MBUTTONUP, 0x10),
        }
    }

    fn inside(size: (i32, i32), x: i32, y: i32) -> Result<(), String> {
        if (0..size.0).contains(&x) && (0..size.1).contains(&y) {
            Ok(())
        } else {
            Err(format!("Le point ({x}, {y}) est hors du viewport ({} x {}) : étape annulée", size.0, size.1))
        }
    }

    /// Plays the steps in the game without Studio coming forward.
    unsafe fn play_behind(view: HWND, size: (i32, i32), steps: &[Step], resolved: &[Vec<Key>]) -> Result<(), String> {
        for (step, keys) in steps.iter().zip(resolved) {
            match step {
                Step::Keys { hold_ms, .. } => {
                    for key in keys {
                        post_key(view, key, false);
                    }
                    sleep(Duration::from_millis(*hold_ms));
                    for key in keys.iter().rev() {
                        post_key(view, key, true);
                    }
                }
                Step::Click { x, y, button } => {
                    inside(size, *x, *y)?;
                    let (down, up, held) = button_messages(*button);
                    // Roblox only registers a hover after a move, and a
                    // button only activates if it was hovered for a few
                    // frames before the press.
                    post_mouse(view, WM_MOUSEMOVE, 0, (*x - 2).max(0), *y);
                    sleep(Duration::from_millis(40));
                    post_mouse(view, WM_MOUSEMOVE, 0, *x, *y);
                    sleep(Duration::from_millis(150));
                    post_mouse(view, down, held, *x, *y);
                    sleep(Duration::from_millis(90));
                    post_mouse(view, up, 0, *x, *y);
                }
                Step::Move { x, y } => {
                    inside(size, *x, *y)?;
                    post_mouse(view, WM_MOUSEMOVE, 0, (*x - 2).max(0), *y);
                    sleep(Duration::from_millis(40));
                    post_mouse(view, WM_MOUSEMOVE, 0, *x, *y);
                }
                Step::Drag { from, to, button, ms } => {
                    inside(size, from.0, from.1)?;
                    inside(size, to.0, to.1)?;
                    let (down, up, held) = button_messages(*button);
                    post_mouse(view, WM_MOUSEMOVE, 0, from.0, from.1);
                    sleep(Duration::from_millis(120));
                    post_mouse(view, down, held, from.0, from.1);
                    // One move per frame or so: a single jump would read
                    // as a click somewhere else, not as a drag.
                    let moves = (*ms / 16).clamp(4, 240) as i32;
                    for index in 1..=moves {
                        let along = |a: i32, b: i32| a + (b - a) * index / moves;
                        post_mouse(view, WM_MOUSEMOVE, held, along(from.0, to.0), along(from.1, to.1));
                        sleep(Duration::from_millis(*ms / moves as u64));
                    }
                    sleep(Duration::from_millis(60));
                    post_mouse(view, up, 0, to.0, to.1);
                }
                Step::Scroll { x, y, notches } => {
                    inside(size, *x, *y)?;
                    post_mouse(view, WM_MOUSEMOVE, 0, *x, *y);
                    sleep(Duration::from_millis(60));
                    // Unlike the other mouse messages, the wheel's position
                    // is given in screen coordinates.
                    let mut origin = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    GetWindowRect(view, &mut origin);
                    let step = if *notches < 0 { -1 } else { 1 };
                    for _ in 0..notches.unsigned_abs().min(40) {
                        let turn = ((120 * step) as i16 as u16 as usize) << 16;
                        post_mouse(view, WM_MOUSEWHEEL, turn, origin.left + *x, origin.top + *y);
                        sleep(Duration::from_millis(30));
                    }
                }
                Step::Wait(ms) => sleep(Duration::from_millis(*ms)),
            }
            sleep(Duration::from_millis(60));
        }
        Ok(())
    }

    /// Presses one of Studio's own shortcuts, such as the one that publishes,
    /// without Studio coming forward: one key, with any of Shift, Ctrl, Alt.
    pub fn press_shortcut(place_name: &str, names: &[String]) -> Result<(), String> {
        const MODIFIERS: [(u16, usize); 3] = [(0x10, 0xA0), (0x11, 0xA2), (VK_ALT, 0xA4)];

        let studio = crate::screenshot::win::find(place_name)?;
        let mut held = Vec::new();
        let mut keys = Vec::new();
        for name in names {
            let virtual_key = virtual_key(name)?;
            let (scan, extended) = scancode(name)?;
            match MODIFIERS.iter().find(|(modifier, _)| *modifier == virtual_key) {
                Some(modifier) => held.push(*modifier),
                None => keys.push(Key { virtual_key, scan, extended }),
            }
        }
        let [key] = keys.as_slice() else {
            return Err("Un raccourci a une seule touche en plus de Maj, Ctrl et Alt".into());
        };

        unsafe {
            if IsWindowEnabled(studio) == 0 {
                return Err("Une boîte de dialogue est ouverte dans Roblox Studio et bloque les entrées : ferme-la d'abord".into());
            }
            // Sharing Studio's input state is what lets this thread write
            // the modifiers where Studio will read them.
            // Studio only acts on a shortcut in a window it believes active.
            // Told so, it does, while the real foreground stays where it is.
            let behind = !in_front(studio);
            if behind {
                PostMessageW(studio, WM_ACTIVATE, 1, 0);
                sleep(Duration::from_millis(300));
            }

            // A worker thread has no message queue until it asks for a
            // message, and without one there is no input state to share.
            let mut message: MSG = std::mem::zeroed();
            PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);

            let current = GetCurrentThreadId();
            let owner = GetWindowThreadProcessId(studio, std::ptr::null_mut());
            if AttachThreadInput(current, owner, 1) == 0 {
                return Err("L'état du clavier de Roblox Studio n'est pas accessible".into());
            }
            let mut before = [0u8; 256];
            GetKeyboardState(before.as_mut_ptr());
            let mut pressed = before;
            for (modifier, left) in &held {
                pressed[*modifier as usize] |= 0x80;
                pressed[*left] |= 0x80;
            }
            SetKeyboardState(pressed.as_ptr());

            let alt = held.iter().any(|(modifier, _)| *modifier == VK_ALT);
            let mut detail = 1u32 | (u32::from(key.scan) << 16);
            if key.extended {
                detail |= 1 << 24;
            }
            if alt {
                detail |= 1 << 29;
            }
            let (down, up) = if alt { (WM_SYSKEYDOWN, WM_SYSKEYUP) } else { (WM_KEYDOWN, WM_KEYUP) };
            PostMessageW(studio, down, key.virtual_key as usize, detail as isize);
            sleep(Duration::from_millis(150));
            PostMessageW(studio, up, key.virtual_key as usize, (detail | 0xC000_0000) as isize);
            // Studio must have read the key before the modifiers go back up.
            sleep(Duration::from_millis(250));

            SetKeyboardState(before.as_ptr());
            AttachThreadInput(current, owner, 0);
            if behind && !in_front(studio) {
                PostMessageW(studio, WM_ACTIVATE, 0, 0);
            }
        }
        Ok(())
    }

    /// Sends the steps to the game of a running test. Studio stays where it
    /// is, in front or behind, and neither the user's keyboard nor their
    /// mouse is touched.
    pub fn run_in_game(place_name: &str, viewport: (i32, i32), steps: &[Step]) -> Result<(), String> {
        let studio = crate::screenshot::win::find(place_name)?;

        // Resolved up front so that a typo can't leave a key held down.
        let mut resolved = Vec::new();
        for step in steps {
            let mut keys = Vec::new();
            if let Step::Keys { keys: names, .. } = step {
                for name in names {
                    let (scan, extended) = scancode(name)?;
                    keys.push(Key { virtual_key: virtual_key(name)?, scan, extended });
                }
            }
            resolved.push(keys);
        }

        unsafe {
            // A modal dialog disables the main window: Studio would swallow
            // every key and click, and the sequence would "succeed" for nothing.
            if IsWindowEnabled(studio) == 0 {
                return Err("Une boîte de dialogue est ouverte dans Roblox Studio et bloque les entrées : ferme-la d'abord".into());
            }
            // A minimized window has no layout, hence no 3D view to find. It
            // comes back without taking the focus.
            if IsIconic(studio) != 0 {
                ShowWindow(studio, SW_SHOWNOACTIVATE);
                sleep(Duration::from_millis(700));
            }
            let (view, _) = viewport_window(studio, viewport)?;
            play_behind(view, viewport, steps, &resolved)
        }
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
        found: Option<(HWND, RECT)>,
    }

    unsafe extern "system" fn match_viewport(handle: HWND, param: LPARAM) -> i32 {
        let search = &mut *(param as *mut Search);
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if IsWindowVisible(handle) != 0 && GetWindowRect(handle, &mut rect) != 0 {
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            if (width - search.size.0).abs() <= 2 && (height - search.size.1).abs() <= 2 {
                // Several nested windows have that size; children come
                // after their parents, so the last one is the innermost,
                // the one the game listens to.
                search.found = Some((handle, rect));
            }
        }
        1
    }

    /// The 3D view is the child window whose size equals the camera's
    /// viewport; nothing else in Studio exposes where it sits on screen.
    unsafe fn viewport_window(studio: HWND, size: (i32, i32)) -> Result<(HWND, RECT), String> {
        let mut search = Search { size, found: None };
        EnumChildWindows(studio, Some(match_viewport), &mut search as *mut _ as LPARAM);
        search
            .found
            .ok_or_else(|| "Viewport de jeu introuvable dans la fenêtre Studio".to_owned())
    }

    unsafe fn viewport_rect(studio: HWND, size: (i32, i32)) -> Result<RECT, String> {
        viewport_window(studio, size).map(|(_, rect)| rect)
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
                Step::Click { x, y, .. } => {
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
                // Only the game takes the rest; Studio's shortcuts are keys.
                Step::Move { .. } | Step::Drag { .. } | Step::Scroll { .. } => {}
            }
            sleep(Duration::from_millis(60));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::{scancode, virtual_key};

        #[test]
        fn every_named_key_has_both_of_its_codes() {
            assert_eq!(virtual_key("w"), Ok(0x57));
            assert_eq!(virtual_key("5"), Ok(0x35));
            assert_eq!(virtual_key("Space"), Ok(0x20));
            assert_eq!(virtual_key("F1"), Ok(0x70));
            assert_eq!(virtual_key("f12"), Ok(0x7B));
            assert_eq!(virtual_key("Left"), Ok(0x25));
            for name in ["a", "z", "0", "9", "space", "enter", "tab", "escape", "backspace", "shift", "ctrl", "alt", "left", "up", "right", "down", "f1", "f10", "f11", "f12"] {
                assert!(scancode(name).is_ok() && virtual_key(name).is_ok(), "{name}");
            }
            for name in ["Banane", "", "F13", "F0", "é", "ab"] {
                assert!(virtual_key(name).is_err(), "{name}");
            }
        }

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

    /// Says whether every key of a shortcut has a name this module knows.
    pub fn check_keys(keys: &[String]) -> Result<(), String> {
        if keys.is_empty() {
            return Err("Le raccourci est vide".into());
        }
        keys.iter().try_for_each(|key| scancode(key).map(|_| ()))
    }

    pub(crate) fn viewport_area(studio: HWND, size: (i32, i32)) -> Result<RECT, String> {
        unsafe { viewport_rect(studio, size) }
    }

    /// Whether Studio has a dialog open: its main window is disabled by a
    /// modal one, or another of its windows is on screen beside it.
    pub fn dialog_open(place_name: &str) -> Result<bool, String> {
        let studio = crate::screenshot::win::find(place_name)?;
        unsafe {
            if IsWindowEnabled(studio) == 0 {
                return Ok(true);
            }
            let mut search = DialogSearch { process: process_of(studio), main: studio, title: None };
            EnumWindows(Some(match_dialog), &mut search as *mut _ as LPARAM);
            Ok(search.title.is_some_and(|title| !title.is_empty()))
        }
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
pub use win::{blocking_dialog, check_keys, dialog_open, press_shortcut, run, run_in_game};

#[cfg(not(windows))]
pub fn check_keys(_keys: &[String]) -> Result<(), String> {
    Ok(())
}

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
pub fn press_shortcut(_place_name: &str, _keys: &[String]) -> Result<(), String> {
    Err("Les entrées clavier et souris ne sont disponibles que sous Windows".into())
}

#[cfg(not(windows))]
pub fn run_in_game(_place_name: &str, _viewport: (i32, i32), _steps: &[Step]) -> Result<(), String> {
    Err("Les entrées clavier et souris ne sont disponibles que sous Windows".into())
}

#[cfg(not(windows))]
pub fn run(_place_name: &str, _viewport: (i32, i32), _steps: &[Step]) -> Result<(), String> {
    Err("Les entrées clavier et souris ne sont disponibles que sous Windows".into())
}
