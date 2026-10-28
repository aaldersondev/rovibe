//! Contact sheets: a burst of captures laid out on one image, each frame
//! stamped with its number and the moment it was taken. A single screenshot
//! can't show an animation, a tween or an effect firing; a row of frames with
//! their times can.

use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, Rgb, RgbImage};

pub const PER_SHEET: usize = 20;
const COLUMNS: u32 = 5;
const TILE_WIDTH: u32 = 320;
const GAP: u32 = 4;
const SCALE: u32 = 3;

/// 3×5 glyphs, one row per entry, high bit on the left.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '#' => [0b101, 0b111, 0b101, 0b111, 0b101],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        's' => [0b000, 0b111, 0b110, 0b011, 0b111],
        _ => [0; 5],
    }
}

/// Writes `text` with its top-left corner at (x, y), on a dark band so it
/// reads over any picture.
fn stamp(image: &mut RgbImage, x: u32, y: u32, text: &str) {
    let advance = 4 * SCALE;
    let width = text.chars().count() as u32 * advance + SCALE;
    let height = 7 * SCALE;
    for dy in 0..height {
        for dx in 0..width {
            let (px, py) = (x + dx, y + dy);
            if px < image.width() && py < image.height() {
                let Rgb([r, g, b]) = *image.get_pixel(px, py);
                image.put_pixel(px, py, Rgb([r / 4, g / 4, b / 4]));
            }
        }
    }
    for (index, c) in text.chars().enumerate() {
        let rows = glyph(c);
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..3 {
                if bits & (0b100 >> column) == 0 {
                    continue;
                }
                for sy in 0..SCALE {
                    for sx in 0..SCALE {
                        let px = x + SCALE + index as u32 * advance + column * SCALE + sx;
                        let py = y + SCALE + row as u32 * SCALE + sy;
                        if px < image.width() && py < image.height() {
                            image.put_pixel(px, py, Rgb([255, 255, 255]));
                        }
                    }
                }
            }
        }
    }
}

/// A frame brought down to the size it will have on the sheet.
pub fn shrink(frame: &RgbImage) -> RgbImage {
    let (width, height) = frame.dimensions();
    if width <= TILE_WIDTH {
        return frame.clone();
    }
    let tile_height = ((height as u64 * TILE_WIDTH as u64) / width.max(1) as u64).max(1) as u32;
    image::imageops::resize(frame, TILE_WIDTH, tile_height, FilterType::Triangle)
}

/// The label of a frame: its number from 1 and its time since the first.
pub fn label(index: usize, seconds: f64) -> String {
    format!("#{} +{:.2}s", index + 1, seconds)
}

/// Lays `frames` out left to right, then top to bottom, `PER_SHEET` to a
/// sheet. `first` is the number of the first frame, for sheets after the
/// first one. Returns one JPEG per sheet.
pub fn build(frames: &[(RgbImage, f64)], first: usize) -> Result<Vec<Vec<u8>>, String> {
    let mut sheets = Vec::new();
    for (chunk_index, chunk) in frames.chunks(PER_SHEET).enumerate() {
        let (width, height) = chunk[0].0.dimensions();
        let tile_height = ((height as u64 * TILE_WIDTH as u64) / width.max(1) as u64).max(1) as u32;
        let columns = COLUMNS.min(chunk.len() as u32);
        let rows = (chunk.len() as u32).div_ceil(COLUMNS);
        let mut sheet = RgbImage::from_pixel(
            columns * TILE_WIDTH + (columns + 1) * GAP,
            rows * tile_height + (rows + 1) * GAP,
            Rgb([24, 24, 28]),
        );
        for (offset, (frame, seconds)) in chunk.iter().enumerate() {
            let tile = image::imageops::resize(frame, TILE_WIDTH, tile_height, FilterType::Triangle);
            let column = offset as u32 % COLUMNS;
            let row = offset as u32 / COLUMNS;
            let x = GAP + column * (TILE_WIDTH + GAP);
            let y = GAP + row * (tile_height + GAP);
            image::imageops::replace(&mut sheet, &tile, x as i64, y as i64);
            let index = first + chunk_index * PER_SHEET + offset;
            stamp(&mut sheet, x + 4, y + 4, &label(index, *seconds));
        }
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 80)
            .encode_image(&sheet)
            .map_err(|error| error.to_string())?;
        sheets.push(jpeg);
    }
    Ok(sheets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_laid_out_twenty_to_a_sheet_and_stamped() {
        let frames: Vec<(RgbImage, f64)> = (0..23)
            .map(|index| (RgbImage::from_pixel(640, 360, Rgb([200, 40, 40])), index as f64 * 0.25))
            .collect();
        let sheets = build(&frames, 0).unwrap();
        assert_eq!(sheets.len(), 2);

        let first = image::load_from_memory(&sheets[0]).unwrap().to_rgb8();
        // Five columns of 320 px with their gaps, four rows of 180 px.
        assert_eq!(first.dimensions(), (5 * 320 + 6 * 4, 4 * 180 + 5 * 4));
        let second = image::load_from_memory(&sheets[1]).unwrap().to_rgb8();
        assert_eq!(second.dimensions(), (3 * 320 + 4 * 4, 180 + 2 * 4));

        // The stamp darkens its band and draws white strokes in it.
        let band = first.get_pixel(4 + 4 + 1, 4 + 4 + 1);
        assert!(band[0] < 120, "bande sombre attendue, {band:?}");
        let has_white = (0..60).any(|dx| (0..21).any(|dy| first.get_pixel(8 + dx, 8 + dy)[1] > 200));
        assert!(has_white, "le texte du tampon doit être visible");
    }

    #[test]
    fn labels_count_from_one_with_the_time_since_the_first_frame() {
        assert_eq!(label(0, 0.0), "#1 +0.00s");
        assert_eq!(label(11, 2.756), "#12 +2.76s");
    }
}
