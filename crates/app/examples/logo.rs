//! Draws the GodCode logo (`</>` on a dark tile) into `assets/`:
//! `cargo run -p godcode --example logo` from the workspace root.

use image::{Rgba, RgbaImage};

const TILE: [u8; 3] = [0x11, 0x11, 0x11];
const MARK: [u8; 3] = [0xF2, 0xF2, 0xF2];
/// Corner radius of the tile and half the stroke width, as parts of the side.
const CORNER: f32 = 0.22;
const HALF_STROKE: f32 = 0.034;
/// The strokes of `<`, `/` and `>`, in a unit square.
const STROKES: [[(f32, f32); 2]; 5] = [
    [(0.36, 0.34), (0.21, 0.50)],
    [(0.21, 0.50), (0.36, 0.66)],
    [(0.56, 0.27), (0.44, 0.73)],
    [(0.64, 0.34), (0.79, 0.50)],
    [(0.79, 0.50), (0.64, 0.66)],
];

fn distance_to_segment(point: (f32, f32), [a, b]: [(f32, f32); 2]) -> f32 {
    let (px, py) = (point.0 - a.0, point.1 - a.1);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let along = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - along * dx).hypot(py - along * dy)
}

/// How far `point` is outside a rounded unit square (negative inside).
fn distance_to_tile(point: (f32, f32)) -> f32 {
    let half = 0.5 - CORNER;
    let x = ((point.0 - 0.5).abs() - half).max(0.0);
    let y = ((point.1 - 0.5).abs() - half).max(0.0);
    x.hypot(y) - CORNER
}

/// The logo at `side` pixels, edges smoothed over one pixel.
fn draw(side: u32) -> RgbaImage {
    let scale = side as f32;
    // Coverage of a pixel whose centre is `distance` (in units) outside a shape.
    let coverage = |distance: f32| (0.5 - distance * scale).clamp(0.0, 1.0);
    RgbaImage::from_fn(side, side, |x, y| {
        let point = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
        let nearest = STROKES
            .iter()
            .map(|stroke| distance_to_segment(point, *stroke))
            .fold(f32::INFINITY, f32::min);
        let ink = coverage(nearest - HALF_STROKE);
        let mix =
            |tile: u8, mark: u8| (f32::from(tile) * (1.0 - ink) + f32::from(mark) * ink) as u8;
        Rgba([
            mix(TILE[0], MARK[0]),
            mix(TILE[1], MARK[1]),
            mix(TILE[2], MARK[2]),
            (coverage(distance_to_tile(point)) * 255.0) as u8,
        ])
    })
}

/// A Windows icon holding the logo at every size the shell asks for.
fn icon() -> Vec<u8> {
    let sizes = [16_u32, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes.iter().map(|side| bitmap(&draw(*side))).collect();
    let mut file = Vec::new();
    file.extend_from_slice(&[0, 0, 1, 0]);
    file.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (side, image) in sizes.iter().zip(&images) {
        // A side of 256 is written as 0.
        let side = *side as u8;
        file.extend_from_slice(&[side, side, 0, 0, 1, 0, 32, 0]);
        file.extend_from_slice(&(image.len() as u32).to_le_bytes());
        file.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in images {
        file.extend_from_slice(&image);
    }
    file
}

/// One image of an icon: a bitmap header, the pixels bottom row first, an empty mask.
fn bitmap(image: &RgbaImage) -> Vec<u8> {
    let side = image.width();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&40_u32.to_le_bytes());
    bytes.extend_from_slice(&(side as i32).to_le_bytes());
    // Twice the height: the pixels, then the mask.
    bytes.extend_from_slice(&(2 * side as i32).to_le_bytes());
    bytes.extend_from_slice(&[1, 0, 32, 0]);
    bytes.extend_from_slice(&[0; 24]);
    for y in (0..side).rev() {
        for x in 0..side {
            let [red, green, blue, alpha] = image.get_pixel(x, y).0;
            bytes.extend_from_slice(&[blue, green, red, alpha]);
        }
    }
    let mask_row = side.div_ceil(32) as usize * 4;
    bytes.resize(bytes.len() + mask_row * side as usize, 0);
    bytes
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    draw(512).save("assets/logo.png")?;
    draw(64).save("assets/favicon.png")?;
    std::fs::write("assets/icon.ico", icon())?;
    println!("wrote assets/logo.png, assets/favicon.png, assets/icon.ico");
    Ok(())
}
