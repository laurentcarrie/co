mod app;
mod bundle;
mod calib;
mod detect;
mod export;
mod help;
mod launcher;
mod model;
mod pdf;
mod place;
mod tcx;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};

/// Visualise an orienteering race on its map. Without arguments, a menu asks for the files.
///
/// A race is saved as one zip file, `<date>-<name>.zip`, with the map, the track of the watch and the
/// transformation that places the track on the map.
#[derive(Parser)]
struct Args {
    /// A saved race (a .zip file), or the map of a new race: a PDF file, or a scan saved as a PNG,
    /// JPEG or TIFF image.
    file: Option<PathBuf>,
    /// For a new race: the Garmin .tcx file. You click the controls on the map to calibrate it.
    #[arg(long, value_name = "FILE")]
    tcx: Option<PathBuf>,
    /// For a new race: its name (default: guessed from the map file name).
    #[arg(long, requires = "tcx")]
    name: Option<String>,
    /// For a new race: where to save the zip (default: `<date>-<name>.zip` next to the .tcx file).
    #[arg(long, requires = "tcx")]
    out: Option<PathBuf>,
    /// For a new race: how many controls the course has, the finish not included. If not given, the
    /// window asks for it.
    #[arg(long, value_name = "N", requires = "tcx", value_parser = clap::value_parser!(u32).range(1..))]
    controls: Option<u32>,
    /// For a new race: ask OpenStreetMap for the name of the place of the track, when no name is given.
    /// This sends one position (the middle of the track, rounded to about 100 m) over the network.
    #[arg(long, requires = "tcx")]
    lookup_place: bool,
    /// Width in pixels used to render the PDF when the scan cannot be extracted
    /// directly (poppler fallback only).
    #[arg(long, default_value_t = 4096)]
    render_width: u32,
}

/// The window: the start menu, then the viewer.
enum Root {
    Menu {
        launcher: Box<launcher::Launcher>,
        render_width: u32,
    },
    Viewer(Box<app::App>),
}

impl eframe::App for Root {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        let choice = match self {
            Root::Viewer(app) => return app.ui(ui, frame),
            Root::Menu { launcher, .. } => launcher.ui(ui),
        };
        let Some(choice) = choice else { return };
        let Root::Menu {
            launcher,
            render_width,
        } = self
        else {
            return;
        };
        let ctx = ui.ctx();
        let opened = match choice {
            launcher::Choice::New(request) => open_new(request, ctx, *render_width),
            launcher::Choice::Open(zip) => open_saved(&zip, ctx, *render_width),
        };
        match opened {
            Ok(app) => *self = Root::Viewer(Box::new(app)),
            Err(e) => launcher.failed(format!("{e:#}")),
        }
    }
}

/// Starts a new race from the files chosen in the menu.
fn open_new(
    request: launcher::Request,
    ctx: &eframe::egui::Context,
    render_width: u32,
) -> Result<app::App> {
    let zip = request.zip()?;
    new_race(
        ctx,
        &request.map,
        &request.tcx,
        request.samples,
        zip,
        request.name,
        None,
        render_width,
    )
}

/// Loads the map and the track, and starts the calibration of a new race. With `controls`, the number of
/// controls is not asked.
#[allow(clippy::too_many_arguments)]
fn new_race(
    ctx: &eframe::egui::Context,
    map_path: &Path,
    tcx_path: &Path,
    samples: Vec<tcx::Sample>,
    zip: PathBuf,
    name: String,
    controls: Option<u32>,
    render_width: u32,
) -> Result<app::App> {
    let map_ext = launcher::map_extension(map_path)
        .context("la carte doit être un fichier PDF, PNG, JPEG ou TIFF")?;
    let map_bytes = std::fs::read(map_path)
        .with_context(|| format!("impossible de lire {}", map_path.display()))?;
    let tcx_bytes = std::fs::read(tcx_path)
        .with_context(|| format!("impossible de lire {}", tcx_path.display()))?;
    let map = pdf::load_map(map_path, render_width)?;
    let native = map
        .native_width
        .context("impossible de connaître la taille en pixels de la carte")?;
    let csv_to_page = map.image.width as f32 / native as f32;
    ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Title(format!(
        "visu-course-co: {name}"
    )));
    let save = app::SaveInfo {
        zip,
        name: name.clone(),
        map: map_bytes,
        map_ext,
        tcx: tcx_bytes,
        native_width: native,
    };
    let calib = app::CalibSession::new(
        samples,
        save,
        controls.unwrap_or(app::DEFAULT_CONTROLS) as usize,
    );
    Ok(app::App::new(
        ctx,
        model::Course::empty(),
        map.image,
        csv_to_page,
        Some(calib),
        Some(name),
        controls.is_none(),
    ))
}

/// Opens a saved race: the map and the track come out of the zip.
fn open_saved(zip: &Path, ctx: &eframe::egui::Context, render_width: u32) -> Result<app::App> {
    let loaded = bundle::load(zip)?;
    let scratch = bundle::Scratch::new()?;
    let map_path = scratch.write(&format!("map.{}", loaded.map_ext), &loaded.map)?;
    let map = pdf::load_map(&map_path, render_width)?;
    let native = map
        .native_width
        .context("impossible de connaître la taille en pixels de la carte")?;
    let csv_to_page = map.image.width as f32 / native as f32;
    let text = std::str::from_utf8(&loaded.tcx).context("race.tcx n'est pas du texte")?;
    let samples = tcx::parse(text)?;
    ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Title(format!(
        "visu-course-co: {}",
        loaded.transformation.race.name
    )));
    app::App::from_bundle(
        ctx,
        loaded,
        zip.to_path_buf(),
        samples,
        map.image,
        csv_to_page,
    )
}

fn main() -> Result<()> {
    let args = Args::parse();
    let options = eframe::NativeOptions::default();
    let render_width = args.render_width;
    let Some(file) = args.file.clone() else {
        anyhow::ensure!(args.tcx.is_none(), "give the map file first");
        return eframe::run_native(
            "visu-course-co",
            options,
            Box::new(move |_| {
                Ok(Box::new(Root::Menu {
                    launcher: Box::new(launcher::Launcher::new()),
                    render_width,
                }))
            }),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    };
    let is_zip = file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if is_zip {
        anyhow::ensure!(args.tcx.is_none(), "a saved race (.zip) needs no --tcx");
        return eframe::run_native(
            "visu-course-co",
            options,
            Box::new(move |cc| {
                let app = open_saved(&file, &cc.egui_ctx, render_width)?;
                Ok(Box::new(Root::Viewer(Box::new(app))))
            }),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    }
    let tcx_path = args
        .tcx
        .clone()
        .context("give either a saved race (.zip) or a map with --tcx <file>")?;
    let samples = tcx::load(&tcx_path)?;
    let date = launcher::date_of(&samples).context("pas de date dans le fichier TCX")?;
    let mut name = args.name.clone().filter(|n| !n.trim().is_empty());
    if name.is_none() && args.lookup_place {
        name = place::center(&samples).and_then(|c| match place::lookup(c) {
            Ok(found) => {
                println!("Place: {found}");
                Some(found)
            }
            Err(e) => {
                eprintln!("note: no place name ({e:#})");
                None
            }
        });
    }
    let name = name.unwrap_or_else(|| launcher::guess_name(&file));
    let zip = match args.out.clone() {
        Some(out) => out,
        None => {
            let dir = tcx_path.parent().unwrap_or(Path::new("."));
            let slug = launcher::slug(&name);
            anyhow::ensure!(!slug.is_empty(), "give a name with --name");
            launcher::zip_path(dir, &date, &slug)
        }
    };
    let controls = args.controls;
    eframe::run_native(
        "visu-course-co",
        options,
        Box::new(move |cc| {
            let app = new_race(
                &cc.egui_ctx,
                &file,
                &tcx_path,
                samples,
                zip,
                name,
                controls,
                render_width,
            )?;
            Ok(Box::new(Root::Viewer(Box::new(app))))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
