//! Export of a race as a PDF: the map with the legs drawn on it, one colour per leg, as in the viewer.

use crate::model::Course;
use crate::pdf::PageImage;
use anyhow::{Context, Result};
use lopdf::{Document, Object, Stream, dictionary};
use std::path::Path;

/// Resolution assumed for the size of the PDF page: the scan of a sheet is made at 300 dpi.
const DPI: f32 = 300.0;

/// Draws the legs on the map, and writes the result as a one-page PDF. `color(i)` is the colour of leg `i`.
/// The part of the track before the triangle is drawn lighter, from the start of the recording.
pub fn race_pdf(
    page: &PageImage,
    course: &Course,
    csv_to_page: f32,
    color: impl Fn(usize) -> [u8; 3],
    path: &Path,
) -> Result<()> {
    let (w, h) = (page.width, page.height);
    let mut img = Canvas { w, h, rgba: page.rgba.clone() };
    let to_page = |p: (f32, f32)| (p.0 * csv_to_page, p.1 * csv_to_page);
    // Line widths in pixels of the page: about 0.6 mm at 300 dpi.
    let width = (w as f32 / 500.0).max(3.0);

    let lead: Vec<(f32, f32)> = course.lead_in.iter().filter_map(|p| p.px).map(to_page).collect();
    let mut ends = Vec::new();
    for (i, leg) in course.legs.iter().enumerate() {
        let pts: Vec<(f32, f32)> = leg.points.iter().filter_map(|p| p.px).map(to_page).collect();
        if i == 0 {
            let mut before = lead.clone();
            before.extend(pts.first().copied());
            img.polyline(&before, width * 0.8, color(0), 0.5);
        }
        img.polyline(&pts, width, color(i), 1.0);
        if let Some(&end) = pts.last() {
            ends.push((end, color(i)));
        }
    }
    for (end, c) in ends {
        img.disc(end, width * 2.4, [255, 255, 255], 1.0);
        img.disc(end, width * 1.8, c, 1.0);
    }
    write_pdf(&img, path)
}

/// The map image, to draw on.
struct Canvas {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

impl Canvas {
    fn put(&mut self, x: i64, y: i64, c: [u8; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        for k in 0..3 {
            self.rgba[i + k] = (self.rgba[i + k] as f32 * (1.0 - alpha) + c[k] as f32 * alpha) as u8;
        }
    }

    fn disc(&mut self, p: (f32, f32), r: f32, c: [u8; 3], alpha: f32) {
        let (x0, y0) = (p.0.round() as i64, p.1.round() as i64);
        let ri = r.ceil() as i64;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                if ((dx * dx + dy * dy) as f32) <= r * r {
                    self.put(x0 + dx, y0 + dy, c, alpha);
                }
            }
        }
    }

    /// A line of the given width. With `alpha` below 1 the line is blended with the map, without darkening where
    /// the discs overlap: each pixel is painted once.
    fn polyline(&mut self, pts: &[(f32, f32)], width: f32, c: [u8; 3], alpha: f32) {
        if pts.len() < 2 {
            return;
        }
        let r = width / 2.0;
        // Pixels already painted by this line.
        let mut done = vec![false; self.w * self.h];
        let mut paint = |canvas: &mut Canvas, x: i64, y: i64| {
            if x >= 0 && y >= 0 && x < canvas.w as i64 && y < canvas.h as i64 {
                let k = y as usize * canvas.w + x as usize;
                if !done[k] {
                    done[k] = true;
                    canvas.put(x, y, c, alpha);
                }
            }
        };
        for seg in pts.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            let steps = (len / (r.max(1.0) * 0.5)).ceil().max(1.0) as usize;
            for s in 0..=steps {
                let t = s as f32 / steps as f32;
                let (cx, cy) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let ri = r.ceil() as i64;
                let (x0, y0) = (cx.round() as i64, cy.round() as i64);
                for dy in -ri..=ri {
                    for dx in -ri..=ri {
                        let (px, py) = ((x0 + dx) as f32 - cx, (y0 + dy) as f32 - cy);
                        if px * px + py * py <= r * r {
                            paint(self, x0 + dx, y0 + dy);
                        }
                    }
                }
            }
        }
    }
}

/// One page, the size of the map at 300 dpi, with the image as a JPEG.
fn write_pdf(img: &Canvas, path: &Path) -> Result<()> {
    let rgb: Vec<u8> = img.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(&rgb, img.w as u32, img.h as u32, image::ExtendedColorType::Rgb8)
        .context("impossible d'encoder l'image")?;

    let (w_pt, h_pt) = (img.w as f32 * 72.0 / DPI, img.h as f32 * 72.0 / DPI);
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let image_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => img.w as i64,
            "Height" => img.h as i64,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpeg,
    ));
    let content = format!("q {w_pt} 0 0 {h_pt} 0 0 cm /Im0 Do Q");
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), w_pt.into(), h_pt.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    doc.save(path)
        .with_context(|| format!("impossible d'écrire {}", path.display()))?;
    Ok(())
}
