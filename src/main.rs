mod app;
mod calib;
mod model;
mod pdf;
mod tcx;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

/// Visualise an orienteering race on its map.
#[derive(Parser)]
struct Args {
    /// PDF file with the race map.
    pdf: PathBuf,
    /// Either the CSV file with the track, or a Garmin .tcx file: in that case you first
    /// click the controls on the map to calibrate, and the CSV is generated.
    input: PathBuf,
    /// Where to write the CSV when `input` is a .tcx file (default: next to it, `*-legs.csv`).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Width in pixels of the page image the CSV coordinates refer to.
    /// Defaults to the size of the scan embedded in the PDF.
    #[arg(long)]
    csv_width: Option<u32>,
    /// Width in pixels used to render the PDF when the scan cannot be extracted
    /// directly (poppler fallback only).
    #[arg(long, default_value_t = 4096)]
    render_width: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let is_tcx = args.input.extension().is_some_and(|e| e.eq_ignore_ascii_case("tcx"));
    let (course, calib) = if is_tcx {
        let samples = tcx::load(&args.input)?;
        let out = args.out.clone().unwrap_or_else(|| {
            let stem = args.input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            args.input.with_file_name(format!("{stem}-legs.csv"))
        });
        (model::Course::empty(), Some(app::CalibSession::new(samples, out)))
    } else {
        (model::Course::load(&args.input)?, None)
    };
    let map = pdf::load_map(&args.pdf, args.render_width)?;
    let csv_width = args
        .csv_width
        .or(map.native_width)
        .context("cannot tell the CSV pixel size: pass --csv-width")?;
    let page = map.image;
    let csv_to_page = page.width as f32 / csv_width as f32;
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "visu-course-co",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, course, page, csv_to_page, calib)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
