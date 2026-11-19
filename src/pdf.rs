//! Loads the map from page 1 of a PDF.
//!
//! The map is normally a scan: one big image on the page. It is extracted
//! directly in pure Rust at its native resolution (no quality loss). If that
//! is not possible (vector map, unusual encoding), it falls back to poppler's
//! `pdftoppm`, when installed.

use anyhow::{Context, Result, bail};
use lopdf::{Document, Object};
use std::path::Path;
use std::process::Command;

pub struct PageImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub struct MapPage {
    pub image: PageImage,
    /// Width in pixels of the scan the CSV coordinates normally refer to.
    pub native_width: Option<u32>,
}

pub fn load_map(pdf: &Path, fallback_width: u32) -> Result<MapPage> {
    match extract_scan(pdf) {
        Ok(image) => {
            let native_width = Some(image.width as u32);
            Ok(MapPage { image, native_width })
        }
        Err(e) => {
            eprintln!("note: cannot extract the scan directly ({e:#}); falling back to pdftoppm");
            let image = render_with_poppler(pdf, fallback_width)?;
            Ok(MapPage { image, native_width: embedded_image_size(pdf).map(|(w, _)| w) })
        }
    }
}

/// Extracts the largest image of page 1 at native resolution.
fn extract_scan(pdf: &Path) -> Result<PageImage> {
    let doc = Document::load(pdf).with_context(|| format!("cannot read {}", pdf.display()))?;
    let page_id = *doc.get_pages().get(&1).context("PDF has no page 1")?;
    let images = doc.get_page_images(page_id).context("cannot list page images")?;
    let best = images.iter().max_by_key(|i| i.width * i.height).context("no image on page 1")?;
    let (w, h) = (best.width as usize, best.height as usize);
    let stream = doc.get_object(best.id)?.as_stream()?;
    let filters: Vec<String> = stream
        .filters()
        .map(|f| f.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect())
        .unwrap_or_default();

    let dynimg = if filters.iter().any(|f| f == "DCTDecode") {
        image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg)
            .context("cannot decode JPEG scan")?
    } else {
        let data = stream.decompressed_content().context("cannot decompress image stream")?;
        match data.len() {
            n if n == w * h * 3 => image::DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(w as u32, h as u32, data).context("bad RGB buffer")?,
            ),
            n if n == w * h => image::DynamicImage::ImageLuma8(
                image::GrayImage::from_raw(w as u32, h as u32, data).context("bad gray buffer")?,
            ),
            n => bail!("unsupported image layout ({n} bytes for {w}x{h}, filters {filters:?})"),
        }
    };

    // Honour a page rotation, if any.
    let rotate = match doc.get_dictionary(page_id)?.get(b"Rotate") {
        Ok(Object::Integer(r)) => r.rem_euclid(360),
        _ => 0,
    };
    let dynimg = match rotate {
        90 => dynimg.rotate90(),
        180 => dynimg.rotate180(),
        270 => dynimg.rotate270(),
        _ => dynimg,
    };
    let rgba = dynimg.to_rgba8();
    Ok(PageImage { width: rgba.width() as usize, height: rgba.height() as usize, rgba: rgba.into_raw() })
}

/// Pixel size of the largest image embedded in page 1, via poppler (fallback only).
fn embedded_image_size(pdf: &Path) -> Option<(u32, u32)> {
    let out = Command::new("pdfimages").args(["-list", "-f", "1", "-l", "1"]).arg(pdf).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(2)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            Some((f.get(3)?.parse::<u32>().ok()?, f.get(4)?.parse::<u32>().ok()?))
        })
        .max_by_key(|(w, h)| w * h)
}

/// Renders page 1 scaled to `width` pixels wide (aspect ratio preserved).
fn render_with_poppler(pdf: &Path, width: u32) -> Result<PageImage> {
    let dir = std::env::temp_dir().join(format!("visu-course-co-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let prefix = dir.join("page");
    let status = Command::new("pdftoppm")
        .args(["-scale-to-x", &width.to_string(), "-scale-to-y", "-1", "-png", "-singlefile", "-f", "1", "-l", "1"])
        .arg(pdf)
        .arg(&prefix)
        .status()
        .context("cannot run `pdftoppm` (install poppler-utils)")?;
    if !status.success() {
        bail!("pdftoppm failed on {}", pdf.display());
    }
    let png = prefix.with_extension("png");
    let img = image::open(&png).with_context(|| format!("cannot read {}", png.display()))?.to_rgba8();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(PageImage { width: img.width() as usize, height: img.height() as usize, rgba: img.into_raw() })
}
