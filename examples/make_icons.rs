//! Renders the icon from its SVG sources into the files the app and the installers use.
//!
//!     cargo run --example make_icons
//!
//! `assets/icon.svg` is the icon; `assets/icon-small.svg` is its simplified drawing for sizes
//! below 32 pixels. The outputs are committed, so this only needs running when a drawing changes.

use resvg::{tiny_skia, usvg};
use std::fs;
use std::path::Path;

/// The drawing that suits `size`, rendered as a square image that it fills edge to edge (Windows, Linux).
fn render(size: u32) -> tiny_skia::Pixmap {
    render_in(size, false)
}

/// The drawing on the macOS icon grid: the rounded square is 824 of 1024 units with an even margin
/// around it, as every other Mac app's icon is, so it doesn't look bigger than them in the Dock,
/// Launchpad and Finder.
fn render_mac(size: u32) -> tiny_skia::Pixmap {
    render_in(size, true)
}

fn render_in(size: u32, mac_grid: bool) -> tiny_skia::Pixmap {
    let source = if size < 32 { "assets/icon-small.svg" } else { "assets/icon.svg" };
    let svg = fs::read_to_string(source).unwrap_or_else(|e| panic!("cannot read {source}: {e}"));
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap_or_else(|e| panic!("{source} is not valid SVG: {e}"));
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("a non-empty image");
    let side = tree.size().width();
    let transform = if mac_grid {
        // The rounded square spans 2..62 of the 64 units in icon.svg, all 64 in icon-small.svg.
        let inset = if size < 32 { 0.0 } else { 2.0 };
        let scale = size as f32 * 824.0 / 1024.0 / (side - 2.0 * inset);
        let offset = size as f32 * 100.0 / 1024.0 - inset * scale;
        tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, offset, offset)
    } else {
        tiny_skia::Transform::from_scale(size as f32 / side, size as f32 / side)
    };
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap
}

fn png(size: u32) -> Vec<u8> {
    render(size).encode_png().expect("PNG encoding")
}

/// An image as Windows icon files hold the sizes below 256: an uncompressed 32-bit bitmap, bottom
/// row first, followed by an (unused, all-opaque) 1-bit mask.
fn bitmap(size: u32) -> Vec<u8> {
    let pixmap = render(size);
    let mut out = Vec::new();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    // The height counts the image and the mask.
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for row in pixmap.pixels().chunks(size as usize).rev() {
        for pixel in row {
            // Stored with straight alpha, in blue-green-red order.
            let color = pixel.demultiply();
            out.extend_from_slice(&[color.blue(), color.green(), color.red(), color.alpha()]);
        }
    }
    let mask_row = size.div_ceil(32) as usize * 4;
    out.resize(out.len() + mask_row * size as usize, 0);
    out
}

/// A Windows icon file with each of the sizes.
fn ico(sizes: &[u32]) -> Vec<u8> {
    // Only the largest size may be stored as a PNG: readers expect plain bitmaps for the rest.
    let images: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&size| (size, if size >= 256 { png(size) } else { bitmap(size) })).collect();
    let mut file = Vec::new();
    file.extend_from_slice(&[0, 0, 1, 0]);
    file.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        // A side of 256 is written as 0.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        file.extend_from_slice(&[side, side, 0, 0, 1, 0, 32, 0]);
        file.extend_from_slice(&(data.len() as u32).to_le_bytes());
        file.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &images {
        file.extend_from_slice(data);
    }
    file
}

/// A macOS icon file: each size as a PNG, under the type codes macOS reads.
fn icns() -> Vec<u8> {
    const ENTRIES: [(&[u8; 4], u32); 10] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"icp6", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic13", 256),
    ];
    let mut body = Vec::new();
    for (code, size) in ENTRIES.iter().chain([(b"ic14", 512)].iter()) {
        let data = render_mac(*size).encode_png().expect("PNG encoding");
        body.extend_from_slice(*code);
        body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut file = b"icns".to_vec();
    file.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    file.extend_from_slice(&body);
    file
}

fn main() {
    let assets = Path::new("assets");
    // The panel's window takes its icon from the PNG; the Windows programs carry the icon file.
    fs::write(assets.join("icon-256.png"), png(256)).expect("writing the PNG");
    fs::write(assets.join("icon.ico"), ico(&[16, 20, 24, 32, 40, 48, 64, 128, 256])).expect("writing the icon file");
    // macOS: the app bundle's icon, and the picture the panel gives the Dock when it runs outside it.
    fs::write(assets.join("icon.icns"), icns()).expect("writing the macOS icon file");
    fs::write(assets.join("icon-macos-512.png"), render_mac(512).encode_png().expect("PNG encoding")).expect("writing the macOS PNG");
    println!("wrote assets/icon-256.png, assets/icon.ico, assets/icon.icns and assets/icon-macos-512.png");
}
