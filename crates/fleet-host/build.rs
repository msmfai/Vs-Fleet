//! Build-time icon generation + Tauri build glue.
//!
//! The single replaceable icon source of truth is `icons/icon.png`; everything
//! else (`32x32.png`, `128x128.png`, `icon.ico`, `Fleet.icns`) is derived here
//! in pure Rust so the build behaves identically on macOS, Linux, and Windows.
//! (`scripts/refresh-icons.sh` remains as a local helper, but the build no
//! longer depends on macOS `sips` or python+Pillow.)

use std::io::Cursor;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=icons/icon.png");

    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
    if let Err(err) = generate_icons(Path::new(&manifest_dir).join("icons")) {
        println!("cargo:warning=icon generation failed: {err}; keeping existing generated icons");
    }

    tauri_build::build();
}

fn generate_icons(icons: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let src = icons.join("icon.png");
    if !src.is_file() {
        return Err(format!("source icon not found: {}", src.display()).into());
    }
    let img = image::open(&src)?.into_rgba8();

    // The PNGs the host embeds (`include_image!` in mux.rs) and Tauri bundles.
    resize(&img, 32).save(icons.join("32x32.png"))?;
    resize(&img, 128).save(icons.join("128x128.png"))?;

    write_ico(&img, &icons.join("icon.ico"))?;
    write_icns(&img, &icons.join("Fleet.icns"))?;
    Ok(())
}

fn resize(img: &image::RgbaImage, size: u32) -> image::RgbaImage {
    image::imageops::resize(img, size, size, image::imageops::FilterType::Lanczos3)
}

fn png_bytes(img: image::RgbaImage) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)?;
    Ok(buf)
}

/// Windows `.ico` with the sizes Explorer/taskbar actually pick from.
fn write_ico(img: &image::RgbaImage, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};

    let mut pngs = Vec::new();
    for size in [16u32, 24, 32, 48, 64, 256] {
        pngs.push((size, png_bytes(resize(img, size))?));
    }
    let frames = pngs
        .iter()
        .map(|(size, png)| {
            IcoFrame::with_encoded(
                png.as_slice(),
                *size,
                *size,
                image::ExtendedColorType::Rgba8,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let file = std::fs::File::create(out)?;
    IcoEncoder::new(file).encode_images(&frames)?;
    Ok(())
}

/// macOS `.icns`: the container format is just a header plus length-prefixed
/// PNG entries, so write it directly rather than pulling in another crate.
fn write_icns(img: &image::RgbaImage, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    const ENTRIES: [(&[u8; 4], u32); 11] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"ic11", 32),
        (b"icp6", 64),
        (b"ic12", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic13", 256),
        (b"ic09", 512),
        (b"ic14", 512),
        (b"ic10", 1024),
    ];

    let mut blobs = Vec::new();
    for (code, size) in ENTRIES {
        blobs.push((code, png_bytes(resize(img, size))?));
    }
    let total: u32 = 8 + blobs
        .iter()
        .map(|(_, data)| 8 + data.len() as u32)
        .sum::<u32>();

    let mut bytes = Vec::with_capacity(total as usize);
    bytes.extend_from_slice(b"icns");
    bytes.extend_from_slice(&total.to_be_bytes());
    for (code, data) in &blobs {
        bytes.extend_from_slice(*code);
        bytes.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(data);
    }
    std::fs::write(out, bytes)?;
    Ok(())
}
