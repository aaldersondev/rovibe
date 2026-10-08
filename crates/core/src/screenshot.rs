//! Captures Roblox Studio's window from the outside. The plugin can't do it:
//! Studio's capture API only exists inside a running client and can't hand
//! pixels to a plugin in the edit DataModel.

use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, RgbImage};

pub struct Capture {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// False when the 3D view couldn't be located and the whole window was
    /// kept instead.
    pub cropped: bool,
}

fn encode(rgb: RgbImage, cropped: bool, max_width: u32) -> Result<Capture, String> {
    let rgb = if rgb.width() > max_width {
        let height = (rgb.height() as u64 * max_width as u64 / rgb.width() as u64) as u32;
        image::imageops::resize(&rgb, max_width, height.max(1), FilterType::Triangle)
    } else {
        rgb
    };

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 82)
        .encode_image(&rgb)
        .map_err(|error| error.to_string())?;

    Ok(Capture {
        jpeg,
        width: rgb.width(),
        height: rgb.height(),
        cropped,
    })
}

#[cfg(windows)]
pub(crate) mod win {
    use std::ffi::c_void;

    use image::RgbImage;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, RECT},
        Graphics::Gdi::{
            CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
            ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
        },
        Storage::Xps::PrintWindow,
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowRect, GetWindowTextW, IsIconic, IsWindowVisible, SetWindowPos,
            ShowWindow, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE,
        },
    };

    const TITLE_SUFFIX: &str = "Roblox Studio";
    /// Asks DWM for the composed content; without it a GPU-rendered window
    /// comes back black.
    const PW_RENDERFULLCONTENT: u32 = 2;

    struct Window {
        handle: HWND,
        title: String,
    }

    unsafe extern "system" fn collect(handle: HWND, param: LPARAM) -> i32 {
        let windows = &mut *(param as *mut Vec<Window>);
        if IsWindowVisible(handle) != 0 {
            let mut buffer = [0u16; 512];
            let length = GetWindowTextW(handle, buffer.as_mut_ptr(), buffer.len() as i32);
            let title = String::from_utf16_lossy(&buffer[..length.max(0) as usize]);
            if title.ends_with(TITLE_SUFFIX) {
                windows.push(Window { handle, title });
            }
        }
        1
    }

    pub(crate) fn find(place_name: &str) -> Result<HWND, String> {
        let mut windows: Vec<Window> = Vec::new();
        unsafe {
            EnumWindows(Some(collect), &mut windows as *mut _ as LPARAM);
        }

        // Studio puts the place name in its title, which is the only link
        // between a plugin connection and a window.
        let matching: Vec<&Window> = windows
            .iter()
            .filter(|window| !place_name.is_empty() && window.title.contains(place_name))
            .collect();

        match (matching.len(), windows.len()) {
            (1, _) => Ok(matching[0].handle),
            (0, 1) => Ok(windows[0].handle),
            (_, 0) => Err("Aucune fenêtre Roblox Studio trouvée".into()),
            _ => Err(format!(
                "Impossible de savoir quelle fenêtre Studio capturer pour « {place_name} »"
            )),
        }
    }

    /// Captures the Studio window, cut down to the 3D view when its size is
    /// given: panels and toolbars are most of the pixels and none of what an
    /// agent is looking for.
    pub fn capture(place_name: &str, viewport: Option<(i32, i32)>) -> Result<(RgbImage, bool), String> {
        let handle = find(place_name)?;

        unsafe {
            if IsIconic(handle) != 0 {
                // A minimized window draws nothing. It is brought back behind
                // everything else and without focus, so the user's own work
                // stays in front, then given time to render a frame.
                ShowWindow(handle, SW_SHOWNOACTIVATE);
                SetWindowPos(handle, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
                std::thread::sleep(std::time::Duration::from_millis(700));
                if IsIconic(handle) != 0 {
                    return Err("Roblox Studio est réduit dans la barre des tâches et n'a pas pu être restauré".into());
                }
            }

            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetWindowRect(handle, &mut rect);
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            if width <= 0 || height <= 0 {
                return Err("La fenêtre Studio n'a pas de taille".into());
            }

            let screen = GetDC(std::ptr::null_mut());
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let previous = SelectObject(memory, bitmap);

            let printed = PrintWindow(handle, memory, PW_RENDERFULLCONTENT);

            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            // Negative height asks for rows from the top down.
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;

            let mut bgra = vec![0u8; width as usize * height as usize * 4];
            let lines = GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                bgra.as_mut_ptr() as *mut c_void,
                &mut info,
                DIB_RGB_COLORS,
            );

            SelectObject(memory, previous);
            DeleteObject(bitmap);
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);

            if printed == 0 || lines == 0 {
                return Err("La capture de la fenêtre Studio a échoué".into());
            }

            let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
            for pixel in bgra.chunks_exact(4) {
                rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            let window = RgbImage::from_raw(width as u32, height as u32, rgb)
                .ok_or_else(|| "Image de capture invalide".to_owned())?;

            let Some(area) = viewport.and_then(|size| crate::input::viewport_area(handle, size).ok()) else {
                return Ok((window, false));
            };
            let x = (area.left - rect.left).clamp(0, width - 1) as u32;
            let y = (area.top - rect.top).clamp(0, height - 1) as u32;
            let crop_width = ((area.right - area.left) as u32).min(width as u32 - x);
            let crop_height = ((area.bottom - area.top) as u32).min(height as u32 - y);
            Ok((
                image::imageops::crop_imm(&window, x, y, crop_width, crop_height).to_image(),
                true,
            ))
        }
    }
}

#[cfg(windows)]
pub fn capture(place_name: &str, viewport: Option<(i32, i32)>, max_width: u32) -> Result<Capture, String> {
    let (image, cropped) = win::capture(place_name, viewport)?;
    encode(image, cropped, max_width)
}

#[cfg(not(windows))]
pub fn capture(_place_name: &str, _viewport: Option<(i32, i32)>, _max_width: u32) -> Result<Capture, String> {
    Err("La capture d'écran n'est disponible que sous Windows".into())
}
