mod app;
mod calib;
mod detect;
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
    /// CSV file with the track (one row per trackpoint), shown as is, without calibration.
    #[arg(long, value_name = "FILE", conflicts_with = "tcx", required_unless_present = "tcx")]
    csv: Option<PathBuf>,
    /// Garmin .tcx file: you first click the controls on the map to calibrate, and a CSV is
    /// generated.
    #[arg(long, value_name = "FILE")]
    tcx: Option<PathBuf>,
    /// Where to write the CSV generated from --tcx (default: next to it, `*-legs.csv`).
    #[arg(long, requires = "tcx")]
    out: Option<PathBuf>,
    /// With --tcx, how many controls the course has, the finish not included.
    #[arg(long, value_name = "N", requires = "tcx", default_value_t = app::DEFAULT_CONTROLS,
          value_parser = clap::value_parser!(u32).range(1..))]
    controls: u32,
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
    anyhow::ensure!(args.out.is_none() || args.tcx.is_some(), "--out only makes sense with --tcx");
    let (course, calib) = match (&args.tcx, &args.csv) {
        (Some(tcx_path), _) => {
            let samples = tcx::load(tcx_path)?;
            let out = args.out.clone().unwrap_or_else(|| {
                let stem = tcx_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                tcx_path.with_file_name(format!("{stem}-legs.csv"))
            });
            (model::Course::empty(), Some(app::CalibSession::new(samples, out, args.controls as usize)))
        }
        (None, Some(csv_path)) => (model::Course::load(csv_path)?, None),
        (None, None) => anyhow::bail!("give either --csv <file> or --tcx <file>"),
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
