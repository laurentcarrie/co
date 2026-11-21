use crate::bundle;
use crate::calib;
use crate::detect;
use crate::export;
use crate::help;
use crate::model::{Course, Leg, Point};
use crate::pdf::PageImage;
use crate::tcx::Sample;
use anyhow::{Context as _, Result};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;

type Click = (f64, f64);

/// Controls proposed when starting, before the user adds or removes some.
pub const DEFAULT_CONTROLS: u32 = 10;

/// Data needed to (re)calibrate a track on the map.
/// Where and with what the race is saved: the zip, and what goes into it besides the transformation.
pub struct SaveInfo {
    pub zip: PathBuf,
    pub name: String,
    pub map: Vec<u8>,
    pub map_ext: String,
    pub tcx: Vec<u8>,
    /// Width in pixels of the map image the saved points refer to.
    pub native_width: u32,
}

pub struct CalibSession {
    pub samples: Vec<Sample>,
    save: SaveInfo,
    /// The sample where the user said they passed the triangle, if they did.
    start_sample: Option<usize>,
    /// Places in page pixels: start triangle, controls 1..n, finish. `None` until clicked.
    slots: Vec<Option<Click>>,
    /// The place the next click on the map goes to.
    selected: Option<usize>,
    message: String,
    /// Numbers of the watch laps the user does not want (pressed by mistake).
    ignored_laps: Vec<usize>,
    /// The last fit: a preview of the route while placing, the saved result afterwards.
    report: Option<calib::Report>,
    /// While the user is placing themselves at the start triangle: the chosen time, in seconds
    /// since the watch start.
    placing: Option<f64>,
    /// What is placed or ignored has changed since the last save.
    unsaved: bool,
    /// The lap button was pressed at the triangle: it is like control 0, tied to a lap press.
    lap_at_triangle: bool,
    /// While placing: the next click on the map moves the start triangle.
    placing_triangle: bool,
    /// The triangle was moved while placing: the saved CSV is out of date.
    triangle_moved: bool,
    /// Empty places are filled from the circles found on the map (at the start, when a control is added, and
    /// when this is ticked again).
    auto_place: bool,
    /// Laps the user dragged: lap number -> sample where the press really was.
    lap_moves: HashMap<usize, usize>,
    /// While a lap is dragged on the map: its number and the sample under the pointer.
    dragging_lap: Option<(usize, usize)>,
    /// While a control point is dragged on the map: its index in `slots`.
    dragging_slot: Option<usize>,
}

impl CalibSession {
    pub fn new(samples: Vec<Sample>, save: SaveInfo, controls: usize) -> Self {
        CalibSession {
            samples,
            save,
            start_sample: None,
            slots: vec![None; controls + 2],
            selected: Some(0),
            ignored_laps: Vec::new(),
            message: String::new(),
            report: None,
            placing: None,
            unsaved: true,
            lap_at_triangle: false,
            placing_triangle: false,
            triangle_moved: false,
            auto_place: true,
            lap_moves: HashMap::new(),
            dragging_lap: None,
            dragging_slot: None,
        }
    }

    /// Back to the start, as when no CSV was found: nothing placed, no lap ignored or moved.
    fn reset(&mut self, controls: usize) {
        self.slots = vec![None; controls + 2];
        self.selected = Some(0);
        self.ignored_laps.clear();
        self.lap_moves.clear();
        self.start_sample = None;
        self.dragging_lap = None;
        self.dragging_slot = None;
        self.lap_at_triangle = false;
        self.auto_place = true;
        self.placing = None;
        self.placing_triangle = false;
        self.triangle_moved = false;
        self.report = None;
        self.message.clear();
        self.unsaved = true;
    }

    /// Looks for the circles on the map again, with the current number of controls, and fills the places that
    /// are still empty. What the user already placed is kept.
    fn place_new_control(&mut self, page: &PageImage) {
        let det = detect::detect(page, self.slots.len() - 2);
        // A found place too close to a placed one is the same circle: leave it empty.
        let too_close = (page.width as f64) * 0.02;
        let placed: Vec<Click> = self.slots.iter().flatten().copied().collect();
        let mut filled = 0;
        for (slot, found) in self.slots.iter_mut().zip(det.slots) {
            if slot.is_none() {
                if let Some(f) = found.filter(|f| {
                    placed
                        .iter()
                        .all(|p| (p.0 - f.0).hypot(p.1 - f.1) > too_close)
                }) {
                    *slot = Some(f);
                    filled += 1;
                }
            }
        }
        self.selected = self.slots.iter().position(Option::is_none);
        self.message = if filled > 0 {
            format!("Placement automatique : {filled} point(s) trouvé(s) sur la carte, à vérifier.")
        } else {
            "Placement automatique : aucun cercle trouvé, cliquez sur la carte pour placer les points.".to_string()
        };
    }

    /// Name of a place: "Triangle", "1", ..., "Finish".
    fn slot_name(&self, k: usize) -> String {
        match k {
            0 => "Triangle".into(),
            k if k + 1 == self.slots.len() => "Arrivée".into(),
            k => k.to_string(),
        }
    }

    /// Short name on the map: "Tri", "1", ..., "F".
    fn slot_tag(&self, k: usize) -> String {
        match k {
            0 => "Tri".into(),
            k if k + 1 == self.slots.len() => "A".into(),
            k => k.to_string(),
        }
    }

    /// First place still to place, after `k` (then from the beginning).
    fn next_unplaced(&self, k: usize) -> Option<usize> {
        (k + 1..self.slots.len())
            .chain(0..k)
            .find(|&i| self.slots[i].is_none())
    }

    /// Ignores a lap press, or takes it back.
    fn toggle_lap(&mut self, id: usize) {
        match self.ignored_laps.iter().position(|&x| x == id) {
            Some(pos) => {
                self.ignored_laps.remove(pos);
            }
            None => self.ignored_laps.push(id),
        }
    }

    /// Fits the track to what is placed, to show the route. Closes the placement of the triangle.
    /// What the user decided and what came out of it, as saved in `transformation.yml`. Positions are in pixels of
    /// the map image (the page pixels divided by `csv_to_page`).
    fn transformation(&self, r: &calib::Report, csv_to_page: f64) -> bundle::Transformation {
        let t_of = |s: usize| self.samples[s.min(self.samples.len() - 1)].t;
        let last = self.slots.len() - 1;
        let points = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(k, c)| {
                let name = match k {
                    0 => "triangle".to_string(),
                    k if k == last => "finish".to_string(),
                    k => k.to_string(),
                };
                c.map(|c| bundle::Point { name, x: c.0 / csv_to_page, y: c.1 / csv_to_page })
            })
            .collect();
        let mut ignored: Vec<usize> = self.ignored_laps.iter().map(|id| id + 1).collect();
        ignored.sort_unstable();
        let (a, b) = (r.sim_a, r.sim_b);
        bundle::Transformation {
            version: bundle::FORMAT_VERSION,
            race: bundle::Race {
                name: self.save.name.clone(),
                date: self.samples.first().and_then(|s| s.time_utc.get(..10)).unwrap_or("").to_string(),
                controls: last - 1,
            },
            sources: bundle::Sources::new(
                &self.save.map_ext,
                &self.save.map,
                self.save.native_width,
                &self.save.tcx,
                self.samples.len(),
            ),
            projection: bundle::Projection { lat0: r.origin.0, lon0: r.origin.1, earth_radius_m: 6_371_000.0 },
            similarity: bundle::Similarity {
                a: [a.0 / csv_to_page, a.1 / csv_to_page],
                b: [b.0 / csv_to_page, b.1 / csv_to_page],
                scale_px_per_m: a.0.hypot(a.1) / csv_to_page,
                rotation_deg: a.1.atan2(a.0).to_degrees(),
            },
            points,
            watch: bundle::Watch {
                lap_at_triangle: self.lap_at_triangle,
                start_time_s: self.start_sample.map(t_of),
                ignored_laps: ignored,
                lap_moves: self.lap_moves.iter().map(|(id, s)| (id + 1, t_of(*s))).collect(),
            },
            visits: bundle::Visits {
                start_s: r.track[r.start_idx].t,
                controls_s: r.visits.iter().map(|&(_, j)| r.track[j].t).collect(),
            },
        }
    }

    /// The session of a saved race, with its legs: places, laps and start come from the transformation, and the
    /// fit is done again from them.
    fn from_bundle(
        loaded: &bundle::Loaded,
        zip: PathBuf,
        samples: Vec<Sample>,
        csv_to_page: f64,
    ) -> Result<(Self, Course)> {
        let t = &loaded.transformation;
        let save = SaveInfo {
            zip,
            name: t.race.name.clone(),
            map: loaded.map.clone(),
            map_ext: loaded.map_ext.clone(),
            tcx: loaded.tcx.clone(),
            native_width: t.sources.map.width_px,
        };
        anyhow::ensure!(
            t.points.len() == t.race.controls + 2,
            "transformation.yml : {} points pour {} postes",
            t.points.len(),
            t.race.controls
        );
        let mut cal = CalibSession::new(samples, save, t.race.controls);
        // The sample nearest to a time in seconds.
        let sample_at = |sec: f64| -> usize {
            let s = &cal.samples;
            let i = s.partition_point(|p| p.t < sec).min(s.len() - 1);
            if i > 0 && sec - s[i - 1].t < s[i].t - sec { i - 1 } else { i }
        };
        let slots = t.points.iter().map(|p| Some((p.x * csv_to_page, p.y * csv_to_page))).collect();
        let lap_moves: HashMap<usize, usize> = t
            .watch
            .lap_moves
            .iter()
            .map(|(n, s)| (n.saturating_sub(1), sample_at(*s)))
            .collect();
        let start_sample = t.watch.start_time_s.map(sample_at);
        cal.slots = slots;
        cal.selected = None;
        cal.lap_at_triangle = t.watch.lap_at_triangle;
        cal.ignored_laps = t.watch.ignored_laps.iter().map(|n| n.saturating_sub(1)).collect();
        cal.lap_moves = lap_moves;
        cal.start_sample = start_sample;
        let report = calib::calibrate(
            &cal.samples,
            &cal.slots,
            csv_to_page,
            cal.start_sample,
            cal.lap_at_triangle,
            &cal.ignored_laps,
            &cal.lap_moves,
            true,
        )?;
        let course = Course::from_csv(report.csv.as_deref().context("pas de tronçons")?)?;
        cal.report = Some(report);
        cal.unsaved = false;
        cal.message = loaded.warnings.join(" ; ");
        Ok((cal, course))
    }

    /// Saves the race: the zip with the map, the track and the transformation.
    fn write_zip(&self, r: &calib::Report, csv_to_page: f64) -> Result<()> {
        let t = self.transformation(r, csv_to_page);
        bundle::save(&self.save.zip, &self.save.map, &self.save.tcx, &t)
    }

    fn refresh_preview(&mut self, csv_to_page: f64) {
        self.placing = None;
        self.placing_triangle = false;
        self.fit_preview(csv_to_page);
    }

    /// Fits the track to what is placed, without leaving the placement of the triangle.
    fn fit_preview(&mut self, csv_to_page: f64) {
        self.unsaved = true;
        if self.slots[0].is_none() || self.slots[self.slots.len() - 1].is_none() {
            self.report = None;
            self.message.clear();
            return;
        }
        match calib::calibrate(
            &self.samples,
            &self.slots,
            csv_to_page,
            self.start_sample,
            self.lap_at_triangle,
            &self.ignored_laps,
            &self.lap_moves,
            false,
        ) {
            Ok(r) => {
                self.report = Some(r);
                self.message.clear();
            }
            Err(e) => {
                if self.placing.is_none() {
                    self.report = None;
                }
                self.message = format!("Erreur : {e:#}");
            }
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum ColorMode {
    Leg,
    Pace,
    HeartRate,
}

#[derive(PartialEq, Clone, Copy)]
enum Selection {
    All,
    Leg(usize),
}

pub struct App {
    course: Course,
    /// The help window is open.
    help_open: bool,
    /// Name of the place of the track, if it was looked up.
    place: Option<String>,
    page_size: Vec2,
    /// Map tiles (each within the GPU texture size limit) with their pixel rect on the page.
    tiles: Vec<(egui::TextureHandle, Rect)>,
    /// Converts CSV pixel coordinates to page-image pixel coordinates.
    csv_to_page: f32,
    selection: Selection,
    mode: ColorMode,
    show_other_legs: bool,
    /// Leg to zoom on at the next frame (set when a leg is selected).
    pending_fit: Option<usize>,
    /// Slider position: metres run since the start of the selected leg.
    cursor_m: f64,
    zoom: f32,
    offset: Vec2,
    fitted: bool,
    /// Colour scale ends (m/s and bpm), set with the sliders. `*_bounds` is the range of the data.
    speed_range: (f64, f64),
    hr_range: (f64, f64),
    speed_bounds: (f64, f64),
    hr_bounds: (f64, f64),
    /// Width of the selected leg line, in points.
    line_width: f32,
    calib: Option<CalibSession>,
    /// The map, kept to look for the circle of a control added later.
    page: PageImage,
    /// True while the user is clicking controls on the map.
    calibrating: bool,
    /// The window asking how many controls the course has is open, with this number.
    asking_controls: Option<u32>,
    /// That window was opened by the "Réinitialiser" button: it can be cancelled.
    asking_cancelable: bool,
    /// What the last export said.
    status: String,
    /// Circles seen on the map, and whether they are drawn.
    scan: Option<detect::Scan>,
    show_circles: bool,
}

impl App {
    pub fn new(
        ctx: &egui::Context,
        course: Course,
        page: PageImage,
        csv_to_page: f32,
        calib: Option<CalibSession>,
        place: Option<String>,
        ask_controls: bool,
    ) -> Self {
        let max_side = ctx.input(|i| i.max_texture_side).max(256);
        let mut tiles = Vec::new();
        for y0 in (0..page.height).step_by(max_side) {
            for x0 in (0..page.width).step_by(max_side) {
                let (w, h) = (
                    (page.width - x0).min(max_side),
                    (page.height - y0).min(max_side),
                );
                let mut buf = Vec::with_capacity(w * h * 4);
                for y in y0..y0 + h {
                    let start = (y * page.width + x0) * 4;
                    buf.extend_from_slice(&page.rgba[start..start + w * 4]);
                }
                let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &buf);
                let tex =
                    ctx.load_texture(format!("map-{x0}-{y0}"), img, egui::TextureOptions::LINEAR);
                tiles.push((
                    tex,
                    Rect::from_min_size(
                        Pos2::new(x0 as f32, y0 as f32),
                        Vec2::new(w as f32, h as f32),
                    ),
                ));
            }
        }
        let speed_range = course.range(|p| (p.speed > 0.3).then_some(p.speed));
        let hr_range = course.range(|p| p.hr);
        let course_is_empty = course.legs.is_empty();
        let asking = calib
            .as_ref()
            .is_some_and(|c| ask_controls && course_is_empty && c.slots.len() >= 2);
        let scan = calib
            .as_ref()
            .filter(|_| course_is_empty)
            .map(|_| detect::scan(&page));
        // The circles seen on the map give the number of controls to suggest.
        let default_n = scan
            .as_ref()
            .and_then(detect::Scan::controls)
            .filter(|_| ask_controls)
            .map_or_else(
                || calib.as_ref().map_or(DEFAULT_CONTROLS, |c| (c.slots.len() - 2) as u32),
                |n| (n as u32).clamp(1, 40),
            );
        let mut app = App {
            course,
            help_open: false,
            place,
            page_size: Vec2::new(page.width as f32, page.height as f32),
            tiles,
            csv_to_page,
            selection: Selection::All,
            mode: ColorMode::Leg,
            show_other_legs: true,
            pending_fit: None,
            cursor_m: 0.0,
            zoom: 1.0,
            offset: Vec2::ZERO,
            fitted: false,
            speed_range,
            hr_range,
            speed_bounds: speed_range,
            hr_bounds: hr_range,
            line_width: 3.5,
            calibrating: calib.is_some() && course_is_empty,
            calib,
            page,
            asking_controls: asking.then_some(default_n),
            asking_cancelable: false,
            status: String::new(),
            scan,
            show_circles: false,
        };
        if !asking && course_is_empty {
            app.first_guess();
        }
        app
    }

    /// Opens a saved race: the places, the laps and the start come from the transformation, and the legs are
    /// computed again from them.
    pub fn from_bundle(
        ctx: &egui::Context,
        loaded: bundle::Loaded,
        zip: PathBuf,
        samples: Vec<Sample>,
        page: PageImage,
        csv_to_page: f32,
    ) -> Result<Self> {
        let (cal, course) = CalibSession::from_bundle(&loaded, zip, samples, csv_to_page as f64)?;
        let name = loaded.transformation.race.name.clone();
        Ok(App::new(ctx, course, page, csv_to_page, Some(cal), Some(name), false))
    }

    /// First guess of the places, from the circles printed on the map. The user can move them.
    fn first_guess(&mut self) {
        let csv_to_page = self.csv_to_page as f64;
        let Some(cal) = self.calib.as_mut() else {
            return;
        };
        let det = detect::detect(&self.page, cal.slots.len() - 2);
        cal.slots = det.slots;
        cal.selected = cal.slots.iter().position(Option::is_none);
        cal.refresh_preview(csv_to_page);
        cal.message = det.note;
        // With everything found, the race is saved right away: the button is then grey until something changes.
        if cal.slots.iter().all(Option::is_some) {
            let note = cal.message.clone();
            self.run_calibration(None);
            if let Some(cal) = self.calib.as_mut() {
                if !cal.message.starts_with("Erreur") {
                    cal.message = format!("{note} {}", cal.message);
                }
            }
        }
    }

    /// Window asking how many controls the course has, before anything is placed.
    fn controls_window(&mut self, ctx: &egui::Context) {
        let guess = self.scan.as_ref().and_then(detect::Scan::controls);
        let cancelable = self.asking_cancelable;
        let Some(n) = self.asking_controls.as_mut() else {
            return;
        };
        let (mut confirmed, mut cancelled) = (false, false);
        egui::Window::new("Nombre de postes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                if !cancelable {
                    ui.label("Nouvelle course : aucun fichier enregistré.");
                } else {
                    ui.label("Tout recommencer : les points placés et les tours modifiés sont effacés.");
                }
                match guess {
                    Some(g) => ui.label(format!(
                        "Cercles vus sur la carte : {g} postes et l'arrivée."
                    )),
                    None => ui.label("Aucun cercle d'arrivée vu sur la carte."),
                };
                ui.label("Combien de postes comporte la course ?");
                ui.label(
                    egui::RichText::new("Sans compter le triangle de départ ni l'arrivée.").weak(),
                );
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(n).range(1..=40));
                    confirmed = ui.button("Valider").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if cancelable {
                        cancelled = ui.button("Annuler").clicked();
                    }
                });
            });
        if cancelled {
            self.asking_controls = None;
            self.asking_cancelable = false;
        } else if confirmed {
            let n = self.asking_controls.take().unwrap_or(DEFAULT_CONTROLS) as usize;
            self.asking_cancelable = false;
            if let Some(cal) = self.calib.as_mut() {
                cal.reset(n);
            }
            self.first_guess();
        }
    }

    /// Writes the CSV and loads it. Stays on the same screen. `start_sample` is the sample where the user passed
    /// the triangle, if they said so (see `open_start_window`).
    fn run_calibration(&mut self, start_sample: Option<usize>) {
        let Some(cal) = self.calib.as_mut() else {
            return;
        };
        let csv_to_page = self.csv_to_page as f64;
        if start_sample.is_some() {
            cal.start_sample = start_sample;
        }
        let result = calib::calibrate(
            &cal.samples,
            &cal.slots,
            csv_to_page,
            cal.start_sample,
            cal.lap_at_triangle,
            &cal.ignored_laps,
            &cal.lap_moves,
            true,
        )
        .and_then(|r| {
            let csv = r.csv.as_deref().context("pas de tronçons")?;
            let course = Course::from_csv(csv)?;
            Ok((r, course))
        })
        .and_then(|(r, course)| {
            cal.write_zip(&r, csv_to_page)?;
            Ok((r, course))
        });
        match result {
            Ok((report, course)) => {
                cal.message = format!("Enregistré : {}", cal.save.zip.display());
                cal.placing = None;
                cal.placing_triangle = false;
                cal.triangle_moved = false;
                cal.report = Some(report);
                cal.unsaved = false;
                self.speed_range = course.range(|p| (p.speed > 0.3).then_some(p.speed));
                self.hr_range = course.range(|p| p.hr);
                self.speed_bounds = self.speed_range;
                self.hr_bounds = self.hr_range;
                self.course = course;
                if !self.calibrating {
                    self.selection = Selection::All;
                    self.fitted = false;
                }
            }
            Err(e) => cal.message = format!("Erreur : {e:#}"),
        }
    }

    /// Leaves the calibration for the viewer, on the last saved CSV.
    fn open_viewer(&mut self) {
        if self.course.legs.is_empty() {
            return;
        }
        self.calibrating = false;
        self.selection = Selection::All;
        self.fitted = false;
    }

    /// Opens the window where the user says when they passed the start triangle.
    fn open_start_window(&mut self) {
        let Some(cal) = self.calib.as_mut() else {
            return;
        };
        let Some(report) = cal
            .report
            .as_ref()
            .filter(|r| r.before_first_control().len() >= 2)
        else {
            return;
        };
        cal.placing = Some(report.track[report.start_idx].t);
        cal.placing_triangle = false;
        cal.triangle_moved = false;
        if !self.course.legs.is_empty() {
            self.pending_fit = Some(0);
        }
    }

    fn calibration_panel(&mut self, ui: &mut egui::Ui) {
        let csv_to_page = self.csv_to_page as f64;
        let Some(cal) = self.calib.as_mut() else {
            return;
        };
        ui.heading("Calibration");
        ui.label("Cliquez sur une ligne, puis sur son emplacement sur la carte. Le tracé s'affiche dès que le triangle et l'arrivée sont placés, et se met à jour à chaque poste. Glissez un point placé pour le corriger : le tracé est recalé quand vous relâchez.");
        ui.separator();
        // What to do next, and the buttons: at the top, so that they are always in view.
        let mut open_window = false;
        match &cal.selected {
            Some(k) => ui.label(
                egui::RichText::new(format!(
                    "Cliquez sur la carte pour placer : {}",
                    cal.slot_name(*k)
                ))
                .strong(),
            ),
            None => ui.label("Cliquez sur une ligne pour placer ou déplacer un point."),
        };
        if let Some(r) = &cal.report {
            ui.label(format!(
                "Erreur : moyenne {:.0} m, maximale {:.0} m",
                r.mean_err_m, r.max_err_m
            ));
        }
        if !cal.message.is_empty() {
            ui.label(&cal.message);
        }
        let has_course = !self.course.legs.is_empty();
        if has_course && cal.unsaved {
            ui.label(
                "Modifications non enregistrées : la visionneuse montre le dernier enregistrement.",
            );
        }
        let known = cal.report.as_ref().is_some_and(|r| r.start_from_lap);
        let can_place = cal.placing.is_none()
            && !known
            && cal
                .report
                .as_ref()
                .is_some_and(|r| r.before_first_control().len() >= 2);
        let why_not = if known {
            "Le bouton Tour a été pressé au triangle : l'heure est connue"
        } else {
            "Placez d'abord le triangle et l'arrivée"
        };
        if ui
            .add_enabled(
                can_place,
                egui::Button::new("Quand suis-je passé au triangle ?"),
            )
            .on_disabled_hover_text(why_not)
            .clicked()
        {
            open_window = true;
        }
        ui.separator();
        let mut changed = false;
        let mut detect_new = false;
        let mut reinit = false;
        ui.horizontal(|ui| {
            let n = cal.slots.len() - 2;
            ui.label(format!("Postes : {n}"));
            if ui
                .add_enabled(n > 1, egui::Button::new("−"))
                .on_hover_text("Retirer le dernier poste")
                .clicked()
            {
                cal.slots.remove(n);
                changed = true;
            }
            if ui
                .button("+")
                .on_hover_text("Ajouter un poste avant l'arrivée")
                .clicked()
            {
                cal.slots.insert(n + 1, None);
                changed = true;
                detect_new = cal.auto_place;
            }
        });
        if ui
            .checkbox(&mut cal.lap_at_triangle, "J'ai appuyé sur Tour au triangle")
            .on_hover_text("Le triangle est alors comme le poste 0 : relié à un appui sur Tour, donc l'heure de passage au triangle est connue.")
            .changed()
        {
            changed = true;
        }
        if ui
            .checkbox(&mut cal.auto_place, "Placement automatique")
            .on_hover_text("Cherche les cercles imprimés sur la carte pour placer les points vides. Les points déjà placés ne bougent pas. En recochant la case, la recherche se relance.")
            .changed()
            && cal.auto_place
        {
            detect_new = true;
        }
        ui.checkbox(&mut self.show_circles, "Voir les cercles détectés")
            .on_hover_text("Dessine sur la carte les cercles vus par la détection : bleu pour les postes, jaune pour l'arrivée.");
        if ui
            .button("Réinitialiser à l'estimation")
            .on_hover_text("Efface tout et recommence comme pour une nouvelle course : nombre de postes deviné sur la carte, puis placement automatique.")
            .clicked()
        {
            reinit = true;
        }
        let mut clear = None;
        egui::CollapsingHeader::new(egui::RichText::new("Liste des postes").strong()).default_open(true).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("slots")
            .max_height((ui.available_height() * 0.42).max(120.0))
            .show(ui, |ui| {
                egui::Grid::new("slots")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for k in 0..cal.slots.len() {
                            let placed = cal.slots[k].is_some();
                            let color = if placed {
                                ui.visuals().text_color()
                            } else {
                                Color32::GRAY
                            };
                            let name = egui::RichText::new(cal.slot_name(k)).color(color);
                            if ui.selectable_label(cal.selected == Some(k), name).clicked() {
                                cal.selected = Some(k);
                            }
                            ui.label(
                                egui::RichText::new(if placed { "placé" } else { "—" })
                                    .color(color),
                            );
                            if placed
                                && ui
                                    .small_button("✕")
                                    .on_hover_text("Retirer ce point")
                                    .clicked()
                            {
                                clear = Some(k);
                            }
                            ui.end_row();
                        }
                    });
            });
        });
        // The lap presses of the watch: each control is tied to one. Untick one pressed by mistake.
        let mut toggle = None;
        let mut reset_moves = false;
        if let Some(r) = &cal.report {
            ui.separator();
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("Tours de la montre ({})", r.laps.len())).strong(),
            )
            .default_open(true)
            .show(ui, |ui| {
            ui.label("Glissez un tour sur la carte pour le déplacer le long de la trace.");
            if !cal.lap_moves.is_empty() && ui.small_button("Remettre les tours à leur place").clicked() {
                reset_moves = true;
            }
            ui.label(if r.lap_based {
                "Chaque poste est relié à un appui sur Tour. Décochez un tour appuyé par erreur (ou faites un clic droit dessus sur la carte)."
            } else {
                "Pas assez de tours dans le fichier : chaque poste est relié au point de la trace le plus proche."
            });
            egui::ScrollArea::vertical()
                .id_salt("laps")
                .max_height((ui.available_height() - 130.0).max(80.0))
                .show(ui, |ui| {
                    egui::Grid::new("laps")
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            for lap in &r.laps {
                                let name = if lap.is_stop {
                                    "Arrêt".to_string()
                                } else {
                                    format!("Tour {}", lap.id + 1)
                                };
                                let mut used = !lap.ignored;
                                if ui
                                    .checkbox(&mut used, format!("{name}  {}", fmt_dur(lap.t)))
                                    .changed()
                                {
                                    toggle = Some(lap.id);
                                }
                                let what = match (lap.slot, lap.ignored) {
                                    (_, true) => egui::RichText::new("ignoré").color(Color32::GRAY),
                                    (Some(k), _) => {
                                        egui::RichText::new(format!("→ {}", cal.slot_name(k)))
                                    }
                                    (None, _) => {
                                        egui::RichText::new("inutilisé").color(Color32::GRAY)
                                    }
                                };
                                ui.label(what);
                                ui.end_row();
                            }
                        });
                });
            });
        }
        if let Some(id) = toggle {
            cal.toggle_lap(id);
            changed = true;
        }
        if reset_moves {
            cal.lap_moves.clear();
            changed = true;
        }
        if reinit {
            let n = self.scan.as_ref().and_then(detect::Scan::controls).unwrap_or(cal.slots.len() - 2);
            self.asking_controls = Some(n.clamp(1, 40) as u32);
            self.asking_cancelable = true;
        }
        if let Some(k) = clear {
            cal.slots[k] = None;
            cal.selected = Some(k);
            changed = true;
        }
        if detect_new {
            cal.place_new_control(&self.page);
        }
        if changed {
            if cal.selected.is_some_and(|k| k >= cal.slots.len()) {
                cal.selected = None;
            }
            cal.refresh_preview(csv_to_page);
        }
        if open_window {
            self.open_start_window();
        }
    }

    /// True while the user is choosing when they passed the start triangle.
    fn placing_start(&self) -> bool {
        self.calib.as_ref().is_some_and(|c| c.placing.is_some())
    }

    fn leg_color(i: usize) -> Color32 {
        let mut h = (i as f32 * 0.382) % 1.0;
        // Not orange or brown: the contour lines of the map are of that colour.
        if (0.03..0.16).contains(&h) {
            h = (h + 0.5) % 1.0;
        }
        let v = if i % 2 == 0 { 0.9 } else { 0.7 };
        egui::ecolor::Hsva::new(h, 1.0, v, 1.0).into()
    }

    fn ramp(t: f64) -> Color32 {
        // blue -> cyan -> green -> yellow -> red
        let t = t.clamp(0.0, 1.0) as f32;
        let stops = [
            (0.0, [40.0, 70.0, 220.0]),
            (0.25, [30.0, 200.0, 220.0]),
            (0.5, [40.0, 190.0, 60.0]),
            (0.75, [240.0, 220.0, 30.0]),
            (1.0, [230.0, 30.0, 30.0]),
        ];
        for w in stops.windows(2) {
            let ((t0, c0), (t1, c1)) = (w[0], w[1]);
            if t <= t1 {
                let f = (t - t0) / (t1 - t0);
                let c = |i: usize| (c0[i] + (c1[i] - c0[i]) * f) as u8;
                return Color32::from_rgb(c(0), c(1), c(2));
            }
        }
        Color32::from_rgb(230, 30, 30)
    }

    fn value_color(&self, p: &Point) -> Color32 {
        match self.mode {
            ColorMode::Leg => Color32::WHITE,
            ColorMode::Pace => {
                let (lo, hi) = self.speed_range;
                Self::ramp((p.speed - lo) / (hi - lo).max(1e-9))
            }
            ColorMode::HeartRate => {
                let (lo, hi) = self.hr_range;
                p.hr.map_or(Color32::GRAY, |h| {
                    Self::ramp((h - lo) / (hi - lo).max(1e-9))
                })
            }
        }
    }

    fn to_screen(&self, rect: Rect, px: (f32, f32)) -> Pos2 {
        let p = Vec2::new(px.0, px.1) * self.csv_to_page;
        rect.min + self.offset + p * self.zoom
    }

    fn fit(&mut self, rect: Rect) {
        self.zoom = (rect.width() / self.page_size.x).min(rect.height() / self.page_size.y);
        self.offset = (rect.size() - self.page_size * self.zoom) / 2.0;
        self.fitted = true;
    }

    /// Zoom and centre the view so that the whole leg fills the map area.
    fn fit_leg(&mut self, rect: Rect, leg: usize) {
        // Leg 1 is shown from the start of the recording, with what was done before the triangle.
        let lead: &[Point] = if leg == 0 { &self.course.lead_in } else { &[] };
        let mut pts = lead
            .iter()
            .chain(self.course.legs[leg].points.iter())
            .filter_map(|p| p.px);
        let Some(first) = pts.next() else { return };
        let to_page = |p: (f32, f32)| Pos2::new(p.0, p.1) * self.csv_to_page;
        let mut bbox = Rect::from_min_max(to_page(first), to_page(first));
        for p in pts {
            bbox.extend_with(to_page(p));
        }
        let margin = 40.0;
        let avail = rect.size() - Vec2::splat(2.0 * margin);
        let zoom = (avail.x / bbox.width().max(1.0)).min(avail.y / bbox.height().max(1.0));
        self.zoom = zoom.clamp(0.05, 20.0);
        self.offset = rect.size() / 2.0 - bbox.center().to_vec2() * self.zoom;
        self.fitted = true;
    }

    /// Big two-state switch at the top left: edit (calibration) or view (viewer).
    fn mode_switch(&mut self, ui: &mut egui::Ui) {
        let can_edit = self.calib.is_some();
        let all_placed = self
            .calib
            .as_ref()
            .is_some_and(|c| c.slots.iter().all(Option::is_some));
        let can_view = !self.course.legs.is_empty() || all_placed;
        let (mut go_edit, mut go_view) = (false, false);
        ui.columns(2, |cols| {
            go_edit = mode_button(
                &mut cols[0],
                "ÉDITION",
                self.calibrating,
                can_edit,
                Color32::from_rgb(214, 110, 20),
                "Placer les points sur la carte (calibration)",
                "Édition indisponible",
            );
            go_view = mode_button(
                &mut cols[1],
                "VISUALISATION",
                !self.calibrating,
                can_view,
                Color32::from_rgb(35, 115, 215),
                "Voir les tronçons : durée, distance, allure, fréquence cardiaque",
                "Placez d'abord tous les points",
            );
        });
        if self.calibrating && !can_view {
            ui.label(
                egui::RichText::new("Placez tous les points pour passer en visualisation")
                    .italics(),
            );
        }
        // Save: active only when something changed since the last save.
        let can_save = self.calib.as_ref().is_some_and(|c| c.unsaved && all_placed);
        let (fill, text) = if can_save {
            (Color32::from_rgb(40, 160, 70), Color32::WHITE)
        } else {
            (Color32::from_gray(60), Color32::GRAY)
        };
        let label = egui::RichText::new("Enregistrer").strong().color(text);
        let button = egui::Button::new(label)
            .fill(fill)
            .min_size(egui::vec2(ui.available_width(), 30.0));
        let save = ui
            .with_layout(egui::Layout::top_down_justified(egui::Align::Center), |ui| {
                ui.add_enabled(can_save, button)
            })
            .inner
            .on_hover_text("Enregistre la course : carte, trace et transformation, dans le fichier zip")
            .on_disabled_hover_text(if all_placed {
                "Tout est enregistré"
            } else {
                "Placez d'abord tous les points"
            })
            .clicked();
        if save {
            self.run_calibration(None);
        }
        let can_export = self.calib.is_some() && !self.course.legs.is_empty();
        let export = ui
            .with_layout(egui::Layout::top_down_justified(egui::Align::Center), |ui| {
                ui.add_enabled(
                    can_export,
                    egui::Button::new("Exporter en PDF").min_size(egui::vec2(ui.available_width(), 26.0)),
                )
            })
            .inner
            .on_hover_text("Enregistre la carte avec la course dessinée, en PDF, à côté du fichier zip")
            .on_disabled_hover_text("Enregistrez d'abord la course")
            .clicked();
        if export {
            self.export_pdf();
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        if go_edit {
            self.calibrating = true;
            if let Some(cal) = self.calib.as_mut() {
                cal.placing = None;
            }
        } else if go_view {
            // Save first when the points were never saved or changed since.
            if self
                .calib
                .as_ref()
                .is_some_and(|c| all_placed && (self.course.legs.is_empty() || c.unsaved))
            {
                self.run_calibration(None);
            }
            self.open_viewer();
        }
        ui.add_space(4.0);
    }

    /// Saves the map with the legs drawn on it as a PDF, next to the zip, with the same name.
    fn export_pdf(&mut self) {
        let Some(cal) = &self.calib else { return };
        let path = cal.save.zip.with_extension("pdf");
        let color = |i: usize| {
            let c = Self::leg_color(i);
            [c.r(), c.g(), c.b()]
        };
        self.status =
            match export::race_pdf(&self.page, &self.course, self.csv_to_page, color, &path) {
                Ok(()) if cal.unsaved => format!(
                    "PDF exporté : {} (sans les modifications non enregistrées)",
                    path.display()
                ),
                Ok(()) => format!("PDF exporté : {}", path.display()),
                Err(e) => format!("Erreur : {e:#}"),
            };
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        self.mode_switch(ui);
        if ui.button("? Aide").clicked() {
            self.help_open = !self.help_open;
        }
        if let Some(name) = &self.place {
            ui.label(egui::RichText::new(format!("Lieu : {name}")).strong());
        }
        if self.calibrating {
            return self.calibration_panel(ui);
        }
        if let Some(cal) = &self.calib {
            if let Some(r) = &cal.report {
                ui.label(format!(
                    "Erreur de calibration : moyenne {:.0} m, maximale {:.0} m",
                    r.mean_err_m, r.max_err_m
                ));
                ui.label(format!(
                    "Échelle : {:.2} px/m, {} points",
                    r.scale_px_per_m, r.rows
                ));
                ui.label(format!(
                    "Triangle : à {:.0} m du début du tronçon 1",
                    r.start_gap_m
                ));
                if !r.lap_based {
                    ui.label("Heures des postes : points de la trace les plus proches (trop peu de tours).");
                }
                let worst = r.errors_m.iter().max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((l, e)) = worst {
                    ui.label(format!("Pire poste : {l} ({e:.0} m)"));
                }
            }
            let can_place = self.calib.as_ref().is_some_and(|c| {
                c.placing.is_none()
                    && c.report
                        .as_ref()
                        .is_some_and(|r| !r.start_from_lap && r.before_first_control().len() >= 2)
            });
            if ui
                .add_enabled(can_place, egui::Button::new("Quand suis-je passé au triangle ?"))
                .on_disabled_hover_text("Indisponible quand le bouton Tour a été pressé au triangle : l'heure est connue")
                .clicked()
            {
                self.open_start_window();
            }
            ui.separator();
        }
        ui.heading("Tronçons");
        if ui
            .selectable_value(&mut self.selection, Selection::All, "Tous les tronçons")
            .clicked()
        {
            self.fitted = false;
        }
        for (i, leg) in self.course.legs.iter().enumerate() {
            let label = egui::RichText::new(format!("→ {}", leg.name)).color(Self::leg_color(i));
            if ui
                .selectable_value(&mut self.selection, Selection::Leg(i), label)
                .clicked()
            {
                self.cursor_m = 0.0;
                self.pending_fit = Some(i);
            }
        }
        ui.separator();
        ui.checkbox(
            &mut self.show_other_legs,
            "Afficher les autres tronçons (atténués)",
        );
        ui.label("Colorier selon");
        ui.radio_value(&mut self.mode, ColorMode::Leg, "Tronçon");
        ui.radio_value(&mut self.mode, ColorMode::Pace, "Allure");
        ui.radio_value(&mut self.mode, ColorMode::HeartRate, "Fréquence cardiaque");
        match self.mode {
            ColorMode::Leg => {}
            ColorMode::Pace => {
                self.legend(
                    ui,
                    "lent",
                    "rapide",
                    fmt_pace(1000.0 / self.speed_range.0),
                    fmt_pace(1000.0 / self.speed_range.1),
                );
                range_sliders(
                    ui,
                    &mut self.speed_range,
                    self.speed_bounds,
                    "Lent",
                    "Rapide",
                    |v| fmt_pace(1000.0 / v.max(0.01)),
                );
            }
            ColorMode::HeartRate => {
                self.legend(
                    ui,
                    "basse",
                    "haute",
                    format!("{:.0} bpm", self.hr_range.0),
                    format!("{:.0} bpm", self.hr_range.1),
                );
                range_sliders(ui, &mut self.hr_range, self.hr_bounds, "Min", "Max", |v| {
                    format!("{v:.0} bpm")
                });
            }
        }
        ui.add(egui::Slider::new(&mut self.line_width, 1.0..=12.0).text("Épaisseur du tracé"));
        ui.separator();
        if let Selection::Leg(i) = self.selection {
            let leg = &self.course.legs[i];
            let s = leg.stats();
            ui.heading(format!("Tronçon → {}", leg.name));
            ui.label(format!("Durée : {}", fmt_dur(s.duration_s)));
            ui.label(format!("Distance courue : {:.0} m", s.run_m));
            match s.bird_m {
                Some(b) => ui.label(format!("Distance à vol d'oiseau : {b:.0} m")),
                None => ui.label("Distance à vol d'oiseau : n/d"),
            };
            if let Some(b) = s.bird_m.filter(|b| *b > 0.0) {
                ui.label(format!("Course / vol d'oiseau : {:.2}", s.run_m / b));
            }
            if s.run_m > 0.0 {
                ui.label(format!(
                    "Allure moyenne : {}",
                    fmt_pace(s.duration_s / s.run_m * 1000.0)
                ));
            }
        } else {
            let (t, d) = self.course.legs.iter().fold((0.0, 0.0), |(t, d), l| {
                let s = l.stats();
                (t + s.duration_s, d + s.run_m)
            });
            ui.heading("Total");
            ui.label(format!("Durée : {}", fmt_dur(t)));
            ui.label(format!("Distance courue : {:.2} km", d / 1000.0));
        }
        ui.separator();
        if ui.button("Ajuster à la fenêtre").clicked() {
            self.fitted = false;
        }
    }

    fn legend(&self, ui: &mut egui::Ui, lo: &str, hi: &str, lo_v: String, hi_v: String) {
        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width().min(200.0), 14.0),
            egui::Sense::hover(),
        );
        let n = 40;
        for k in 0..n {
            let x0 = rect.left() + rect.width() * k as f32 / n as f32;
            let x1 = rect.left() + rect.width() * (k + 1) as f32 / n as f32;
            ui.painter().rect_filled(
                Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())),
                0.0,
                Self::ramp(k as f64 / (n - 1) as f64),
            );
        }
        ui.horizontal(|ui| {
            ui.label(format!("{lo}: {lo_v}"));
            ui.label(format!("{hi}: {hi_v}"));
        });
    }

    /// Window to say when the start triangle was passed.
    fn placement_window(&mut self, ctx: &egui::Context) {
        let Some(cal) = self.calib.as_mut() else {
            return;
        };
        let (Some(mut t), Some(report)) = (cal.placing, cal.report.as_ref()) else {
            return;
        };
        let before = report.before_first_control();
        let (Some(first), Some(last)) = (before.first(), before.last()) else {
            return;
        };
        let (t0, t1) = (first.t, last.t);
        let (mut done, mut cancel) = (false, false);
        let mut armed = cal.placing_triangle;
        let message = if cal.message.starts_with("Erreur") {
            cal.message.clone()
        } else {
            String::new()
        };
        egui::Window::new("Quand êtes-vous passé au triangle de départ ?")
            .anchor(egui::Align2::CENTER_TOP, [0.0, 10.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Déplacez le curseur au moment où vous étiez au triangle : le point de la trace à cet instant est placé sur le triangle, et le tronçon 1 est tracé à partir de là.");
                ui.spacing_mut().slider_width = 420.0;
                ui.add(
                    egui::Slider::new(&mut t, t0..=t1)
                        .show_value(false)
                        .text(format!("départ de la montre {} → poste 1 {}", fmt_dur(0.0), fmt_dur(t1 - t0))),
                );
                ui.label(format!("Triangle à {} après le départ de la montre ; le tronçon 1 dure {}.", fmt_dur(t - t0), fmt_dur(t1 - t)));
                ui.horizontal(|ui| {
                    if ui.selectable_label(armed, "Placer le triangle").on_hover_text("Puis cliquez sur son emplacement sur la carte").clicked() {
                        armed = !armed;
                    }
                    if armed {
                        ui.label(egui::RichText::new("Cliquez sur la carte").strong());
                    }
                });
                if !message.is_empty() {
                    ui.label(&message);
                }
                ui.horizontal(|ui| {
                    done = ui.button("Terminé").clicked();
                    cancel = ui.button("Annuler").clicked();
                });
            });
        cal.placing = Some(t);
        cal.placing_triangle = armed;
        if done {
            // Last GPS point at or before the chosen time.
            let sample = before.iter().rev().find(|p| p.t <= t).map(|p| p.sample);
            self.run_calibration(sample);
        } else if cancel {
            cal.placing = None;
            cal.placing_triangle = false;
            if cal.triangle_moved && !self.calibrating {
                // The triangle moved: save again, with the automatic start.
                if let Some(cal) = self.calib.as_mut() {
                    cal.start_sample = None;
                }
                self.run_calibration(None);
            }
        }
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        let Selection::Leg(i) = self.selection else {
            ui.label("Choisissez un tronçon pour le parcourir.");
            return;
        };
        let leg: &Leg = &self.course.legs[i];
        let run = leg.run_m().max(0.0);
        self.cursor_m = self.cursor_m.clamp(0.0, run);
        if run > 0.0 {
            ui.spacing_mut().slider_width = (ui.available_width() - 120.0).max(100.0);
            ui.add(
                egui::Slider::new(&mut self.cursor_m, 0.0..=run)
                    .suffix(" m")
                    .fixed_decimals(0),
            );
        }
        let p = &leg.points[leg.index_at(self.cursor_m)];
        let t0 = leg.points[0].t;
        ui.horizontal(|ui| {
            ui.label(format!("t = {}", fmt_dur(p.t - t0)));
            ui.separator();
            ui.label(format!("distance = {:.0} m", p.dist - leg.points[0].dist));
            ui.separator();
            ui.label(format!(
                "allure = {}",
                p.pace_s_km().map_or("—".into(), fmt_pace)
            ));
            ui.separator();
            ui.label(format!(
                "altitude = {}",
                p.ele.map_or("—".into(), |e| format!("{e:.0} m"))
            ));
            ui.separator();
            ui.label(format!(
                "fréquence cardiaque = {}",
                p.hr.map_or("—".into(), |h| format!("{h:.0} bpm"))
            ));
        });
        // Elevation profile with a cursor marker.
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 50.0), egui::Sense::hover());
        let eles: Vec<f64> = leg.points.iter().filter_map(|p| p.ele).collect();
        if eles.len() > 1 {
            let (lo, hi) = eles
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), e| (a.min(*e), b.max(*e)));
            let d0 = leg.points[0].dist;
            let span = run.max(1.0) as f32;
            let pt = |d: f64, e: f64| {
                Pos2::new(
                    rect.left() + rect.width() * (d - d0) as f32 / span,
                    rect.bottom() - rect.height() * ((e - lo) / (hi - lo).max(1.0)) as f32,
                )
            };
            let line: Vec<Pos2> = leg
                .points
                .iter()
                .filter_map(|p| p.ele.map(|e| pt(p.dist, e)))
                .collect();
            ui.painter()
                .add(egui::Shape::line(line, Stroke::new(1.5, Color32::GRAY)));
            let x = rect.left() + rect.width() * self.cursor_m as f32 / span;
            ui.painter().line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(1.0, Color32::RED),
            );
        }
    }

    /// While the user is choosing when they passed the start triangle: a click on the map can move the
    /// triangle, and leg 1 is drawn from the chosen time. Returns whether it drew (nothing otherwise).
    fn placement_view(
        &mut self,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) -> bool {
        if !self.placing_start() {
            return false;
        }
        // While placing the start triangle, the next click on the map moves it.
        if resp.clicked() {
            let (offset, zoom, page_size, csv_to_page) = (
                self.offset,
                self.zoom,
                self.page_size,
                self.csv_to_page as f64,
            );
            if let (Some(pos), Some(cal)) = (resp.interact_pointer_pos(), self.calib.as_mut()) {
                let p = (pos - rect.min - offset) / zoom;
                if cal.placing.is_some()
                    && cal.placing_triangle
                    && p.x >= 0.0
                    && p.y >= 0.0
                    && p.x <= page_size.x
                    && p.y <= page_size.y
                {
                    cal.slots[0] = Some((p.x as f64, p.y as f64));
                    cal.placing_triangle = false;
                    cal.triangle_moved = true;
                    cal.fit_preview(csv_to_page);
                }
            }
        }

        if let Some(cal) = &self.calib {
            if let (Some(t), Some(report)) = (cal.placing, &cal.report) {
                let pts = report.before_first_control();
                let screen = |p: (f64, f64)| {
                    rect.min + self.offset + Vec2::new(p.0 as f32, p.1 as f32) * self.zoom
                };
                // The track point at the chosen time is put on the triangle, and the drift is spread
                // up to control 1, as it will be in the saved file.
                let last = pts.len() - 1;
                let k = pts
                    .partition_point(|p| p.t <= t)
                    .saturating_sub(1)
                    .min(last.saturating_sub(1));
                let tri = cal.slots[0].unwrap_or(pts[k].page);
                let o_start = (tri.0 - pts[k].raw.0, tri.1 - pts[k].raw.1);
                let o_c1 = (
                    pts[last].page.0 - pts[last].raw.0,
                    pts[last].page.1 - pts[last].raw.1,
                );
                let placed = |j: usize| {
                    let f = if j <= k {
                        0.0
                    } else {
                        (j - k) as f64 / (last - k).max(1) as f64
                    };
                    (
                        pts[j].raw.0 + o_start.0 + (o_c1.0 - o_start.0) * f,
                        pts[j].raw.1 + o_start.1 + (o_c1.1 - o_start.1) * f,
                    )
                };
                // The whole track before control 1, faint; leg 1 from the chosen time, strong.
                let all: Vec<Pos2> = (0..pts.len()).map(|j| screen(placed(j))).collect();
                painter.add(egui::Shape::line(
                    all,
                    Stroke::new(1.5, Color32::from_gray(150)),
                ));
                let before: Vec<Pos2> = (0..=k).map(|j| screen(placed(j))).collect();
                painter.add(egui::Shape::line(
                    before,
                    Stroke::new(2.5, Self::leg_color(0).gamma_multiply(0.5)),
                ));
                let line: Vec<Pos2> = (k..=last).map(|j| screen(placed(j))).collect();
                painter.add(egui::Shape::line(
                    line,
                    Stroke::new(3.5, Self::leg_color(0)),
                ));
                let p = screen(tri);
                painter.circle_filled(p, 4.0, Color32::from_rgb(200, 0, 200));
                painter.text(
                    p + Vec2::new(8.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                    "Tri",
                    egui::FontId::proportional(16.0),
                    Color32::from_rgb(120, 0, 160),
                );
                painter.circle_filled(p, 7.0, Color32::BLACK);
                painter.circle_filled(p, 5.0, Color32::from_rgb(255, 230, 0));
                return true;
            }
        }
        false
    }

    /// Dragging a lap press of the watch (used or not) along the track: the press is moved to the track point
    /// nearest to the pointer, between its two neighbours. The fit is redone when the button is released.
    fn lap_drag(&mut self, resp: &egui::Response, rect: Rect) {
        if !self.calibrating || self.placing_start() {
            return;
        }
        let (offset, zoom, page_size) = (self.offset, self.zoom, self.page_size);
        let csv_to_page = self.csv_to_page as f64;
        let Some(cal) = self.calib.as_mut() else {
            return;
        };
        let screen = |c: Click| rect.min + offset + Vec2::new(c.0 as f32, c.1 as f32) * zoom;
        // A placed point (triangle, control, finish) is dragged to correct its place; the fit is redone when
        // the button is released. With Shift held, the press goes to a lap press of the watch instead (they
        // sit on the controls once fitted).
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let origin = resp.ctx.input(|i| i.pointer.press_origin());
            let shift = resp.ctx.input(|i| i.modifiers.shift);
            if let (Some(o), false) = (origin, shift) {
                cal.dragging_slot = cal
                    .slots
                    .iter()
                    .enumerate()
                    .filter_map(|(k, c)| c.map(|c| (k, screen(c).distance(o))))
                    .filter(|(_, d)| *d < 10.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(k, _)| k);
            }
        }
        if let (Some(k), true) = (cal.dragging_slot, resp.dragged()) {
            if let Some(pos) = resp.interact_pointer_pos() {
                let p = (pos - rect.min - offset) / zoom;
                cal.slots[k] = Some((
                    p.x.clamp(0.0, page_size.x) as f64,
                    p.y.clamp(0.0, page_size.y) as f64,
                ));
            }
        }
        if resp.drag_stopped() && cal.dragging_slot.take().is_some() {
            cal.refresh_preview(csv_to_page);
            return;
        }
        if cal.dragging_slot.is_some() {
            return;
        }
        let Some(r) = &cal.report else {
            return;
        };
        let mut started = None;
        let mut moved = None;
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let origin = resp.ctx.input(|i| i.pointer.press_origin());
            started = origin.and_then(|o| {
                r.laps
                    .iter()
                    .map(|l| (l, screen(l.page).distance(o)))
                    .filter(|(_, d)| *d < 10.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(l, _)| (l.id, l.sample))
            });
        }
        if let (Some((id, _)), true) = (cal.dragging_lap, resp.dragged()) {
            if let (Some(pos), Some(idx)) =
                (resp.interact_pointer_pos(), r.laps.iter().position(|l| l.id == id))
            {
                let target = (pos - rect.min - offset) / zoom;
                let lo = if idx > 0 { r.laps[idx - 1].sample + 1 } else { 1 };
                let hi = r.laps.get(idx + 1).map_or(usize::MAX, |l| l.sample.saturating_sub(1));
                let dist = |p: &calib::TrackPoint| {
                    let (dx, dy) = (p.page.0 as f32 - target.x, p.page.1 as f32 - target.y);
                    dx * dx + dy * dy
                };
                moved = r
                    .track
                    .iter()
                    .filter(|p| p.sample >= lo && p.sample <= hi)
                    .min_by(|a, b| dist(a).total_cmp(&dist(b)))
                    .map(|p| (id, p.sample));
            }
        }
        if started.is_some() {
            cal.dragging_lap = started;
        }
        if moved.is_some() {
            cal.dragging_lap = moved;
        }
        if resp.drag_stopped() {
            if let Some((id, sample)) = cal.dragging_lap.take() {
                cal.lap_moves.insert(id, sample);
                cal.refresh_preview(csv_to_page);
            }
        }
    }

    /// Places the selected point where the map is clicked, and draws the placed points and the route.
    fn calibration_overlay(&mut self, resp: &egui::Response, painter: &egui::Painter, rect: Rect) {
        let csv_to_page = self.csv_to_page as f64;
        let (offset, zoom, page_size) = (self.offset, self.zoom, self.page_size);
        let Some(cal) = self.calib.as_mut() else {
            return;
        };
        if resp.clicked() {
            if let (Some(pos), Some(k)) = (resp.interact_pointer_pos(), cal.selected) {
                let p = (pos - rect.min - offset) / zoom;
                if p.x >= 0.0 && p.y >= 0.0 && p.x <= page_size.x && p.y <= page_size.y {
                    cal.slots[k] = Some((p.x as f64, p.y as f64));
                    cal.selected = cal.next_unplaced(k);
                    cal.refresh_preview(csv_to_page);
                }
            }
        }
        let screen = |c: Click| rect.min + offset + Vec2::new(c.0 as f32, c.1 as f32) * zoom;
        // The route, from the start of leg 1 to the last placed point, one colour per leg.
        if let Some(r) = &cal.report {
            // The whole recording, faint, including what is before the triangle and after the finish.
            let all: Vec<Pos2> = r.track.iter().map(|p| screen(p.page)).collect();
            painter.add(egui::Shape::line(
                all,
                Stroke::new(1.5, Color32::from_rgba_unmultiplied(110, 110, 110, 150)),
            ));
            // What was recorded before the triangle belongs to leg 1: drawn lighter, from the start.
            let before: Vec<Pos2> = r.track[..=r.start_idx]
                .iter()
                .map(|p| screen(p.page))
                .collect();
            painter.add(egui::Shape::line(
                before,
                Stroke::new(2.5, Self::leg_color(0).gamma_multiply(0.5)),
            ));
            let mut from = r.start_idx;
            for &(slot, to) in &r.visits {
                let line: Vec<Pos2> = r.track[from..=to].iter().map(|p| screen(p.page)).collect();
                painter.add(egui::Shape::line(
                    line,
                    Stroke::new(3.0, Self::leg_color(slot - 1)),
                ));
                from = to;
            }
        }
        // The watch lap presses, on the route: right-click one to ignore it (or take it back).
        let mut toggle = None;
        if let Some(r) = &cal.report {
            let hover = resp.hover_pos();
            for lap in &r.laps {
                let dragged = cal.dragging_lap.filter(|(id, _)| *id == lap.id);
                let p = match dragged {
                    Some((_, s)) => {
                        let k = r.track.partition_point(|t| t.sample < s).min(r.track.len() - 1);
                        screen(r.track[k].page)
                    }
                    None => screen(lap.page),
                };
                let (fill, tag) = match (lap.slot, lap.ignored) {
                    (_, true) => (Color32::from_rgb(220, 40, 40), "✕".to_string()),
                    (Some(k), _) => (Color32::WHITE, cal.slot_tag(k)),
                    (None, _) => (Color32::from_gray(150), "?".to_string()),
                };
                let grow = if dragged.is_some() { 3.0 } else { 0.0 };
                painter.circle_filled(p, 6.5 + grow, Color32::BLACK);
                painter.circle_filled(p, 5.0 + grow, fill);
                painter.text(
                    p + Vec2::new(8.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    tag,
                    egui::FontId::proportional(12.0),
                    Color32::BLACK,
                );
                if hover.is_some_and(|h| h.distance(p) < 9.0) {
                    let name = if lap.is_stop {
                        "Arrêt".to_string()
                    } else {
                        format!("Tour {}", lap.id + 1)
                    };
                    let what = match (lap.slot, lap.ignored) {
                        (_, true) => "ignoré".to_string(),
                        (Some(k), _) => format!("→ {}", cal.slot_name(k)),
                        (None, _) => "inutilisé".to_string(),
                    };
                    let text = format!(
                        "{name} à {}, {what} (glisser : déplacer, clic droit : {})",
                        fmt_dur(lap.t),
                        if lap.ignored { "utiliser" } else { "ignorer" }
                    );
                    let galley = painter.layout_no_wrap(
                        text,
                        egui::FontId::proportional(13.0),
                        Color32::WHITE,
                    );
                    let rect =
                        Rect::from_min_size(p + Vec2::new(10.0, -26.0), galley.size()).expand(3.0);
                    painter.rect_filled(rect, 3.0, Color32::from_black_alpha(210));
                    painter.galley(rect.min + Vec2::splat(3.0), galley, Color32::WHITE);
                    if resp.secondary_clicked() {
                        toggle = Some(lap.id);
                    }
                }
            }
        }
        if let Some(id) = toggle {
            cal.toggle_lap(id);
            cal.refresh_preview(csv_to_page);
        }
        if let (true, Some(scan)) = (self.show_circles, &self.scan) {
            for (i, c) in scan.circles.iter().enumerate() {
                let color = if scan.finish == Some(i) {
                    Color32::from_rgb(255, 210, 0)
                } else {
                    Color32::from_rgb(0, 150, 255)
                };
                painter.circle_stroke(
                    screen((c.x as f64, c.y as f64)),
                    c.r * zoom,
                    Stroke::new(2.5, color),
                );
            }
        }
        let magenta = Color32::from_rgb(200, 0, 200);
        let placed: Vec<Pos2> = cal.slots.iter().flatten().map(|c| screen(*c)).collect();
        painter.add(egui::Shape::line(
            placed,
            Stroke::new(1.0, magenta.gamma_multiply(0.6)),
        ));
        for (k, c) in cal.slots.iter().enumerate() {
            let Some(c) = c else { continue };
            let p = screen(*c);
            let radius = if cal.selected == Some(k) { 6.0 } else { 4.0 };
            painter.circle_filled(p, radius, magenta);
            painter.text(
                p + Vec2::new(8.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                cal.slot_tag(k),
                egui::FontId::proportional(16.0),
                Color32::from_rgb(120, 0, 160),
            );
        }
    }

    fn map_view(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = resp.rect;
        if !self.fitted {
            self.fit(rect);
        }
        if let Some(leg) = self.pending_fit.take() {
            self.fit_leg(rect, leg);
        }
        self.lap_drag(&resp, rect);
        let lap_dragging = self
            .calib
            .as_ref()
            .is_some_and(|c| c.dragging_lap.is_some() || c.dragging_slot.is_some());
        if resp.dragged() && !lap_dragging {
            self.offset += resp.drag_delta();
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let factor = (scroll * 0.002).exp();
                if let Some(m) = resp.hover_pos() {
                    // Keep the point under the cursor fixed while zooming.
                    let rel = m - rect.min - self.offset;
                    self.zoom *= factor;
                    self.offset = m - rect.min - rel * factor;
                }
            }
        }
        painter.rect_filled(rect, 0.0, Color32::from_gray(40));
        let painter = painter.with_clip_rect(rect);
        for (tex, tile) in &self.tiles {
            let r = Rect::from_min_max(
                rect.min + self.offset + tile.min.to_vec2() * self.zoom,
                rect.min + self.offset + tile.max.to_vec2() * self.zoom,
            );
            painter.image(
                tex.id(),
                r,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        if self.calibrating {
            if !self.placement_view(&resp, &painter, rect) {
                self.calibration_overlay(&resp, &painter, rect);
            }
            return;
        }

        for (i, leg) in self.course.legs.iter().enumerate() {
            let selected = match self.selection {
                Selection::All => true,
                Selection::Leg(s) => s == i,
            };
            if !selected && !self.show_other_legs {
                continue;
            }
            // While placing the triangle, leg 1 is drawn from the chosen time instead (see below).
            let drawn_apart = i == 0 && self.placing_start();
            let width = if selected {
                self.line_width
            } else {
                self.line_width * 0.57
            };
            let faded = |c: Color32| if selected { c } else { c.gamma_multiply(0.3) };
            // Leg 1 starts at the beginning of the recording: what was done before the triangle is drawn lighter,
            // in every colouring.
            if i == 0 && !drawn_apart {
                let mut lead: Vec<Pos2> = self
                    .course
                    .lead_in
                    .iter()
                    .filter_map(|p| p.px.map(|px| self.to_screen(rect, px)))
                    .collect();
                if let Some(first) = leg.points.iter().find_map(|p| p.px) {
                    lead.push(self.to_screen(rect, first));
                }
                painter.add(egui::Shape::line(
                    lead,
                    Stroke::new(width * 0.8, faded(Self::leg_color(0)).gamma_multiply(0.5)),
                ));
            }
            let pts: Vec<(&Point, Pos2)> = leg
                .points
                .iter()
                .filter_map(|p| p.px.map(|px| (p, self.to_screen(rect, px))))
                .collect();
            if self.mode == ColorMode::Leg || !selected {
                let line: Vec<Pos2> = pts.iter().map(|(_, s)| *s).collect();
                if !drawn_apart {
                    painter.add(egui::Shape::line(
                        line,
                        Stroke::new(width, faded(Self::leg_color(i))),
                    ));
                }
            } else if !drawn_apart {
                for w in pts.windows(2) {
                    painter.line_segment(
                        [w[0].1, w[1].1],
                        Stroke::new(width + 0.5, self.value_color(w[1].0)),
                    );
                }
            }
            if let Some((_, end)) = pts.last() {
                painter.circle_filled(*end, 6.0, Color32::WHITE);
                painter.circle_filled(*end, 4.5, faded(Self::leg_color(i)));
            }
        }

        if self.placement_view(&resp, &painter, rect) {
            return;
        }

        if let Selection::Leg(i) = self.selection {
            let leg = &self.course.legs[i];
            if let Some(px) = leg.px_at(self.cursor_m) {
                let c = self.to_screen(rect, px);
                painter.circle_filled(c, 7.0, Color32::BLACK);
                painter.circle_filled(c, 5.0, Color32::from_rgb(255, 230, 0));
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.asking_controls.is_some() && !self.asking_cancelable {
            egui::CentralPanel::default().show(ui, |_| {});
            self.controls_window(&ui.ctx().clone());
            return;
        }
        egui::Panel::left("side")
            .resizable(false)
            .show(ui, |ui| self.side_panel(ui));
        if !self.calibrating {
            egui::Panel::bottom("bottom").show(ui, |ui| self.bottom_panel(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| self.map_view(ui));
        self.placement_window(&ui.ctx().clone());
        let page = if self.calibrating {
            help::Page::Calibration
        } else {
            help::Page::Viewer
        };
        help::window(ui.ctx(), &mut self.help_open, page);
        if self.asking_cancelable {
            self.controls_window(&ui.ctx().clone());
        }
    }
}

/// One track with two handles for the ends of a colour scale. The handles block each other: min never goes above
/// max and max never goes below min. Below, the two values and a reset button.
fn range_sliders(
    ui: &mut egui::Ui,
    range: &mut (f64, f64),
    bounds: (f64, f64),
    lo_label: &str,
    hi_label: &str,
    fmt: impl Fn(f64) -> String,
) {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().min(220.0), 26.0),
        egui::Sense::click_and_drag(),
    );
    let (x0, x1) = (rect.left() + 9.0, rect.right() - 9.0);
    let span = (bounds.1 - bounds.0).max(1e-9);
    let to_x = |v: f64| x0 + ((v - bounds.0) / span).clamp(0.0, 1.0) as f32 * (x1 - x0);
    let to_v = |x: f32| bounds.0 + (((x - x0) / (x1 - x0)).clamp(0.0, 1.0) as f64) * span;
    // Which handle is dragged is chosen when the drag starts: the nearest one. When they overlap, the one that can move.
    let id = resp.id.with("active");
    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            let (dl, dh) = ((p.x - to_x(range.0)).abs(), (p.x - to_x(range.1)).abs());
            let pick_lo = if dl != dh {
                dl < dh
            } else {
                p.x < to_x(range.0) || range.1 >= bounds.1
            };
            ui.data_mut(|d| d.insert_temp(id, pick_lo));
        }
    }
    if resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos() {
            let pick_lo = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
            let v = to_v(p.x);
            if pick_lo {
                range.0 = v.min(range.1);
            } else {
                range.1 = v.max(range.0);
            }
        }
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    let y = rect.center().y;
    painter.line_segment(
        [Pos2::new(x0, y), Pos2::new(x1, y)],
        Stroke::new(4.0, Color32::from_gray(70)),
    );
    let (xl, xh) = (to_x(range.0), to_x(range.1));
    let n = 24;
    for k in 0..n {
        let a = xl + (xh - xl) * k as f32 / n as f32;
        let b = xl + (xh - xl) * (k + 1) as f32 / n as f32;
        painter.line_segment(
            [Pos2::new(a, y), Pos2::new(b, y)],
            Stroke::new(4.0, App::ramp(k as f64 / (n - 1) as f64)),
        );
    }
    for x in [xl, xh] {
        painter.circle_filled(Pos2::new(x, y), 8.0, Color32::WHITE);
        painter.circle_stroke(
            Pos2::new(x, y),
            8.0,
            Stroke::new(1.5, Color32::from_gray(60)),
        );
    }
    ui.horizontal(|ui| {
        ui.label(format!("{lo_label} : {}", fmt(range.0)));
        ui.label(format!("{hi_label} : {}", fmt(range.1)));
    });
    if ui.small_button("Réinitialiser l'échelle").clicked() {
        *range = bounds;
    }
}

/// One half of the mode switch. The active mode is filled with its colour. Returns true when the user clicks the
/// mode that is not active.
fn mode_button(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    enabled: bool,
    color: Color32,
    tip: &str,
    why_not: &str,
) -> bool {
    let text = egui::RichText::new(label).size(16.0).strong();
    let (text, fill) = if active {
        (text.color(Color32::WHITE), color)
    } else {
        (text.color(Color32::GRAY), Color32::from_gray(45))
    };
    let button = egui::Button::new(text)
        .fill(fill)
        .min_size(egui::vec2(ui.available_width(), 52.0));
    // The columns lay widgets out left-aligned: centre the text of the button.
    let resp = ui
        .with_layout(
            egui::Layout::top_down_justified(egui::Align::Center),
            |ui| ui.add_enabled(enabled, button),
        )
        .inner
        .on_hover_text(tip)
        .on_disabled_hover_text(why_not);
    resp.clicked() && !active
}

fn fmt_dur(s: f64) -> String {
    let s = s.round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn fmt_pace(s_per_km: f64) -> String {
    let s = s_per_km.round() as i64;
    format!("{}:{:02} /km", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is saved is enough to get the same legs back.
    #[test]
    fn saved_race_gives_the_same_legs() {
        let (samples, clicks) = calib::tests::l_track(&[20, 40, 50, 60, 75, 95]);
        let zip = std::env::temp_dir().join("vcc-app-test.zip");
        let save = SaveInfo {
            zip: zip.clone(),
            name: "Test".into(),
            map: b"map".to_vec(),
            map_ext: "pdf".into(),
            tcx: b"tcx".to_vec(),
            native_width: 100,
        };
        let mut cal = CalibSession::new(samples.clone(), save, 3);
        cal.slots = clicks.into_iter().map(Some).collect();
        cal.ignored_laps = vec![3];
        cal.lap_moves = HashMap::from([(4, 70)]);
        cal.start_sample = Some(18);
        let run = |cal: &CalibSession| {
            calib::calibrate(
                &cal.samples,
                &cal.slots,
                1.0,
                cal.start_sample,
                cal.lap_at_triangle,
                &cal.ignored_laps,
                &cal.lap_moves,
                true,
            )
            .unwrap()
        };
        let first = run(&cal);
        cal.write_zip(&first, 1.0).unwrap();

        let loaded = bundle::load(&zip).unwrap();
        assert!(loaded.warnings.is_empty());
        // The visits and the start, in seconds, are in the file.
        let t = &loaded.transformation;
        assert_eq!(t.visits.controls_s.len(), 4);
        assert_eq!(t.visits.start_s, samples[18].t);
        assert_eq!(t.watch.ignored_laps, [4]);
        assert_eq!(t.watch.lap_moves[&5], samples[70].t);
        let (back, course) = CalibSession::from_bundle(&loaded, zip.clone(), samples, 1.0).unwrap();
        let second = back.report.as_ref().unwrap();
        assert_eq!(first.csv, second.csv);
        assert_eq!(course.legs.len(), 4);
        assert_eq!(back.ignored_laps, cal.ignored_laps);
        assert_eq!(back.lap_moves, cal.lap_moves);
        assert_eq!(back.start_sample, cal.start_sample);
        let _ = std::fs::remove_file(&zip);
    }

    /// On a real race: the guess, saved and opened again, gives the same legs. Run with:
    /// VCC_PDF=map.pdf VCC_TCX=race.tcx VCC_N=15 cargo test --release real_zip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_zip() {
        let (pdf, tcx) = (std::env::var("VCC_PDF").unwrap(), std::env::var("VCC_TCX").unwrap());
        let n: usize = std::env::var("VCC_N").unwrap().parse().unwrap();
        let map = crate::pdf::load_map(std::path::Path::new(&pdf), 4096).unwrap();
        let native = map.native_width.unwrap();
        let csv_to_page = map.image.width as f64 / native as f64;
        let samples = crate::tcx::load(std::path::Path::new(&tcx)).unwrap();
        let zip = std::env::temp_dir().join("vcc-real.zip");
        let save = SaveInfo {
            zip: zip.clone(),
            name: "Real".into(),
            map: std::fs::read(&pdf).unwrap(),
            map_ext: "pdf".into(),
            tcx: std::fs::read(&tcx).unwrap(),
            native_width: native,
        };
        let mut cal = CalibSession::new(samples.clone(), save, n);
        cal.slots = detect::detect(&map.image, n).slots;
        let r = calib::calibrate(&cal.samples, &cal.slots, csv_to_page, None, false, &[], &HashMap::new(), true).unwrap();
        cal.write_zip(&r, csv_to_page).unwrap();
        println!("zip: {} bytes", std::fs::metadata(&zip).unwrap().len());
        let loaded = bundle::load(&zip).unwrap();
        println!("{}", serde_yaml::to_string(&loaded.transformation).unwrap());
        let (back, _) = CalibSession::from_bundle(&loaded, zip, samples, csv_to_page).unwrap();
        assert_eq!(r.csv, back.report.as_ref().unwrap().csv);
        println!("same legs after reopening");
    }

    /// The PDF export on a real race. Run with the same variables as `real_zip`; writes VCC_OUT (default
    /// /tmp/vcc-export.pdf).
    #[test]
    #[ignore]
    fn real_export() {
        let (pdf, tcx) = (std::env::var("VCC_PDF").unwrap(), std::env::var("VCC_TCX").unwrap());
        let n: usize = std::env::var("VCC_N").unwrap().parse().unwrap();
        let map = crate::pdf::load_map(std::path::Path::new(&pdf), 4096).unwrap();
        let csv_to_page = map.image.width as f32 / map.native_width.unwrap() as f32;
        let samples = crate::tcx::load(std::path::Path::new(&tcx)).unwrap();
        let slots = detect::detect(&map.image, n).slots;
        let r = calib::calibrate(&samples, &slots, csv_to_page as f64, None, false, &[], &HashMap::new(), true).unwrap();
        let course = Course::from_csv(r.csv.as_deref().unwrap()).unwrap();
        let out = std::env::var("VCC_OUT").unwrap_or("/tmp/vcc-export.pdf".into());
        let color = |i: usize| {
            let c = App::leg_color(i);
            [c.r(), c.g(), c.b()]
        };
        let t = std::time::Instant::now();
        export::race_pdf(&map.image, &course, csv_to_page, color, std::path::Path::new(&out)).unwrap();
        println!("export took {:?}", t.elapsed());
    }
}
