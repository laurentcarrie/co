use crate::calib;
use crate::detect;
use crate::model::{Course, Leg, Point};
use crate::pdf::PageImage;
use crate::tcx::Sample;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::path::PathBuf;

type Click = (f64, f64);

/// Controls proposed when starting, before the user adds or removes some.
pub const DEFAULT_CONTROLS: u32 = 10;

/// Data needed to (re)calibrate a track on the map.
pub struct CalibSession {
    pub samples: Vec<Sample>,
    pub out: PathBuf,
    /// Places in page pixels: start triangle, controls 1..n, finish. `None` until clicked.
    slots: Vec<Option<Click>>,
    /// The place the next click on the map goes to.
    selected: Option<usize>,
    message: String,
    /// The last fit: a preview of the route while placing, the saved result afterwards.
    report: Option<calib::Report>,
    /// While the user is placing themselves at the start triangle: the chosen time, in seconds
    /// since the watch start.
    placing: Option<f64>,
}

impl CalibSession {
    pub fn new(samples: Vec<Sample>, out: PathBuf, controls: usize) -> Self {
        CalibSession {
            samples,
            out,
            slots: vec![None; controls + 2],
            selected: Some(0),
            message: String::new(),
            report: None,
            placing: None,
        }
    }

    /// Name of a place: "Start", "1", ..., "Finish".
    fn slot_name(&self, k: usize) -> String {
        match k {
            0 => "Start".into(),
            k if k + 1 == self.slots.len() => "Finish".into(),
            k => k.to_string(),
        }
    }

    /// Short name on the map: "S", "1", ..., "F".
    fn slot_tag(&self, k: usize) -> String {
        match k {
            0 => "S".into(),
            k if k + 1 == self.slots.len() => "F".into(),
            k => k.to_string(),
        }
    }

    /// First place still to place, after `k` (then from the beginning).
    fn next_unplaced(&self, k: usize) -> Option<usize> {
        (k + 1..self.slots.len()).chain(0..k).find(|&i| self.slots[i].is_none())
    }

    /// Fits the track to what is placed, to show the route. Needs the triangle and the finish.
    fn refresh_preview(&mut self, csv_to_page: f64) {
        self.placing = None;
        if self.slots[0].is_none() || self.slots[self.slots.len() - 1].is_none() {
            self.report = None;
            self.message.clear();
            return;
        }
        match calib::calibrate(&self.samples, &self.slots, csv_to_page, None, None) {
            Ok(r) => {
                self.report = Some(r);
                self.message.clear();
            }
            Err(e) => {
                self.report = None;
                self.message = format!("Error: {e:#}");
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
    speed_range: (f64, f64),
    hr_range: (f64, f64),
    calib: Option<CalibSession>,
    /// True while the user is clicking controls on the map.
    calibrating: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        course: Course,
        page: PageImage,
        csv_to_page: f32,
        calib: Option<CalibSession>,
    ) -> Self {
        let max_side = cc.egui_ctx.input(|i| i.max_texture_side).max(256);
        let mut tiles = Vec::new();
        for y0 in (0..page.height).step_by(max_side) {
            for x0 in (0..page.width).step_by(max_side) {
                let (w, h) = ((page.width - x0).min(max_side), (page.height - y0).min(max_side));
                let mut buf = Vec::with_capacity(w * h * 4);
                for y in y0..y0 + h {
                    let start = (y * page.width + x0) * 4;
                    buf.extend_from_slice(&page.rgba[start..start + w * 4]);
                }
                let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &buf);
                let tex = cc.egui_ctx.load_texture(format!("map-{x0}-{y0}"), img, egui::TextureOptions::LINEAR);
                tiles.push((tex, Rect::from_min_size(Pos2::new(x0 as f32, y0 as f32), Vec2::new(w as f32, h as f32))));
            }
        }
        let speed_range = course.range(|p| (p.speed > 0.3).then_some(p.speed));
        let hr_range = course.range(|p| p.hr);
        let course_is_empty = course.legs.is_empty();
        let mut calib = calib;
        if let Some(cal) = calib.as_mut().filter(|_| course_is_empty) {
            // First guess of the places, from the circles printed on the map. The user can move them.
            let det = detect::detect(&page, cal.slots.len() - 2);
            cal.slots = det.slots;
            cal.selected = cal.slots.iter().position(Option::is_none);
            cal.refresh_preview(csv_to_page as f64);
            cal.message = det.note;
        }
        App {
            course,
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
            calibrating: calib.is_some() && course_is_empty,
            calib,
        }
    }

    /// Writes the CSV and switches to the viewer. `start_sample` is the sample where the user passed
    /// the triangle, once they have said so: until then the placement window opens.
    fn run_calibration(&mut self, start_sample: Option<usize>) {
        let Some(cal) = self.calib.as_mut() else { return };
        let csv_to_page = self.csv_to_page as f64;
        match calib::calibrate(&cal.samples, &cal.slots, csv_to_page, start_sample, Some(&cal.out))
            .and_then(|r| Course::load(&cal.out).map(|c| (r, c)))
        {
            Ok((report, course)) => {
                cal.message = format!("Saved {}", cal.out.display());
                cal.placing = (start_sample.is_none() && report.before_first_control().len() >= 2)
                    .then(|| report.track[report.start_idx].t);
                if cal.placing.is_some() {
                    self.pending_fit = Some(0);
                }
                cal.report = Some(report);
                self.speed_range = course.range(|p| (p.speed > 0.3).then_some(p.speed));
                self.hr_range = course.range(|p| p.hr);
                self.course = course;
                self.selection = Selection::All;
                self.calibrating = false;
                self.fitted = false;
            }
            Err(e) => cal.message = format!("Error: {e:#}"),
        }
    }

    fn calibration_panel(&mut self, ui: &mut egui::Ui) {
        let csv_to_page = self.csv_to_page as f64;
        let Some(cal) = self.calib.as_mut() else { return };
        ui.heading("Calibration");
        ui.label("Click a row, then click its place on the map. The route is drawn once the start and the finish are placed, and updated with each control.");
        ui.separator();
        let mut changed = false;
        ui.horizontal(|ui| {
            let n = cal.slots.len() - 2;
            ui.label(format!("Controls: {n}"));
            if ui.add_enabled(n > 1, egui::Button::new("−")).on_hover_text("Remove the last control").clicked() {
                cal.slots.remove(n);
                changed = true;
            }
            if ui.button("+").on_hover_text("Add a control before the finish").clicked() {
                cal.slots.insert(n + 1, None);
                changed = true;
            }
        });
        let mut clear = None;
        egui::ScrollArea::vertical().max_height(ui.available_height() - 170.0).show(ui, |ui| {
            egui::Grid::new("slots").num_columns(3).striped(true).show(ui, |ui| {
                for k in 0..cal.slots.len() {
                    let placed = cal.slots[k].is_some();
                    let color = if placed { ui.visuals().text_color() } else { Color32::GRAY };
                    let name = egui::RichText::new(cal.slot_name(k)).color(color);
                    if ui.selectable_label(cal.selected == Some(k), name).clicked() {
                        cal.selected = Some(k);
                    }
                    ui.label(egui::RichText::new(if placed { "placed" } else { "—" }).color(color));
                    if placed && ui.small_button("✕").on_hover_text("Remove this point").clicked() {
                        clear = Some(k);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(k) = clear {
            cal.slots[k] = None;
            cal.selected = Some(k);
            changed = true;
        }
        if changed {
            if cal.selected.is_some_and(|k| k >= cal.slots.len()) {
                cal.selected = None;
            }
            cal.refresh_preview(csv_to_page);
        }
        ui.separator();
        match &cal.selected {
            Some(k) => ui.label(egui::RichText::new(format!("Click on the map to place: {}", cal.slot_name(*k))).strong()),
            None => ui.label("Click a row to place or move a point."),
        };
        if let Some(r) = &cal.report {
            ui.label(format!("Error: mean {:.0} m, max {:.0} m", r.mean_err_m, r.max_err_m));
        }
        if !cal.message.is_empty() {
            ui.label(&cal.message);
        }
        let all = cal.slots.iter().all(Option::is_some);
        let compute = ui
            .add_enabled(all, egui::Button::new("Save CSV and view"))
            .on_disabled_hover_text("Place every point first");
        if compute.clicked() {
            self.run_calibration(None);
        }
        if self.course.legs.len() > 0 && ui.button("Back to viewer").clicked() {
            self.calibrating = false;
        }
    }

    /// True while the user is choosing when they passed the start triangle.
    fn placing_start(&self) -> bool {
        self.calib.as_ref().is_some_and(|c| c.placing.is_some())
    }

    fn leg_color(i: usize) -> Color32 {
        let h = (i as f32 * 0.382) % 1.0;
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
                p.hr.map_or(Color32::GRAY, |h| Self::ramp((h - lo) / (hi - lo).max(1e-9)))
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
        let mut pts = self.course.legs[leg].points.iter().filter_map(|p| p.px);
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

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        if self.calibrating {
            return self.calibration_panel(ui);
        }
        if let Some(cal) = &self.calib {
            if let Some(r) = &cal.report {
                ui.label(format!("Calibration error: mean {:.0} m, max {:.0} m", r.mean_err_m, r.max_err_m));
                ui.label(format!("Scale: {:.2} px/m, {} points", r.scale_px_per_m, r.rows));
                ui.label(format!("Start triangle: {:.0} m from the start of leg 1", r.start_gap_m));
                let worst = r.errors_m.iter().max_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((l, e)) = worst {
                    ui.label(format!("Worst control: {l} ({e:.0} m)"));
                }
            }
            if ui.button("Recalibrate").clicked() {
                self.calibrating = true;
                if let Some(cal) = self.calib.as_mut() {
                    cal.placing = None;
                }
            }
            ui.separator();
        }
        ui.heading("Legs");
        if ui.selectable_value(&mut self.selection, Selection::All, "All legs").clicked() {
            self.fitted = false;
        }
        for (i, leg) in self.course.legs.iter().enumerate() {
            let label = egui::RichText::new(format!("→ {}", leg.name)).color(Self::leg_color(i));
            if ui.selectable_value(&mut self.selection, Selection::Leg(i), label).clicked() {
                self.cursor_m = 0.0;
                self.pending_fit = Some(i);
            }
        }
        ui.separator();
        ui.checkbox(&mut self.show_other_legs, "Show other legs (faded)");
        ui.label("Colour by");
        ui.radio_value(&mut self.mode, ColorMode::Leg, "Leg");
        ui.radio_value(&mut self.mode, ColorMode::Pace, "Pace");
        ui.radio_value(&mut self.mode, ColorMode::HeartRate, "Heart rate");
        match self.mode {
            ColorMode::Leg => {}
            ColorMode::Pace => self.legend(ui, "slow", "fast", fmt_pace(1000.0 / self.speed_range.0), fmt_pace(1000.0 / self.speed_range.1)),
            ColorMode::HeartRate => {
                self.legend(ui, "low", "high", format!("{:.0} bpm", self.hr_range.0), format!("{:.0} bpm", self.hr_range.1))
            }
        }
        ui.separator();
        if let Selection::Leg(i) = self.selection {
            let leg = &self.course.legs[i];
            let s = leg.stats();
            ui.heading(format!("Leg → {}", leg.name));
            ui.label(format!("Duration: {}", fmt_dur(s.duration_s)));
            ui.label(format!("Run length: {:.0} m", s.run_m));
            match s.bird_m {
                Some(b) => ui.label(format!("Bird length: {b:.0} m")),
                None => ui.label("Bird length: n/a"),
            };
            if let Some(b) = s.bird_m.filter(|b| *b > 0.0) {
                ui.label(format!("Run / bird: {:.2}", s.run_m / b));
            }
            if s.run_m > 0.0 {
                ui.label(format!("Avg pace: {}", fmt_pace(s.duration_s / s.run_m * 1000.0)));
            }
        } else {
            let (t, d) = self.course.legs.iter().fold((0.0, 0.0), |(t, d), l| {
                let s = l.stats();
                (t + s.duration_s, d + s.run_m)
            });
            ui.heading("Total");
            ui.label(format!("Duration: {}", fmt_dur(t)));
            ui.label(format!("Run length: {:.2} km", d / 1000.0));
        }
        ui.separator();
        if ui.button("Fit to window").clicked() {
            self.fitted = false;
        }
    }

    fn legend(&self, ui: &mut egui::Ui, lo: &str, hi: &str, lo_v: String, hi_v: String) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(200.0), 14.0), egui::Sense::hover());
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
        let Some(cal) = self.calib.as_mut() else { return };
        let (Some(mut t), Some(report)) = (cal.placing, cal.report.as_ref()) else { return };
        let before = report.before_first_control();
        let (Some(first), Some(last)) = (before.first(), before.last()) else { return };
        let (t0, t1) = (first.t, last.t);
        let (mut done, mut cancel) = (false, false);
        egui::Window::new("When did you pass the start triangle?")
            .anchor(egui::Align2::CENTER_TOP, [0.0, 10.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Move the slider to the moment you were at the triangle: leg 1 is drawn from there.");
                ui.spacing_mut().slider_width = 420.0;
                ui.add(
                    egui::Slider::new(&mut t, t0..=t1)
                        .show_value(false)
                        .text(format!("watch start {} → control 1 {}", fmt_dur(0.0), fmt_dur(t1 - t0))),
                );
                ui.label(format!("Triangle at {} after the watch start; leg 1 lasts {}.", fmt_dur(t - t0), fmt_dur(t1 - t)));
                ui.horizontal(|ui| {
                    done = ui.button("Done").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        cal.placing = Some(t);
        if done {
            // Last GPS point at or before the chosen time.
            let sample = before.iter().rev().find(|p| p.t <= t).map(|p| p.sample);
            self.run_calibration(sample);
        } else if cancel {
            cal.placing = None;
        }
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        let Selection::Leg(i) = self.selection else {
            ui.label("Select a leg to scrub through it.");
            return;
        };
        let leg: &Leg = &self.course.legs[i];
        let run = leg.run_m().max(0.0);
        self.cursor_m = self.cursor_m.clamp(0.0, run);
        if run > 0.0 {
            ui.spacing_mut().slider_width = (ui.available_width() - 120.0).max(100.0);
            ui.add(egui::Slider::new(&mut self.cursor_m, 0.0..=run).suffix(" m").fixed_decimals(0));
        }
        let p = &leg.points[leg.index_at(self.cursor_m)];
        let t0 = leg.points[0].t;
        ui.horizontal(|ui| {
            ui.label(format!("t = {}", fmt_dur(p.t - t0)));
            ui.separator();
            ui.label(format!("dist = {:.0} m", p.dist - leg.points[0].dist));
            ui.separator();
            ui.label(format!("pace = {}", p.pace_s_km().map_or("—".into(), fmt_pace)));
            ui.separator();
            ui.label(format!("elevation = {}", p.ele.map_or("—".into(), |e| format!("{e:.0} m"))));
            ui.separator();
            ui.label(format!("heart rate = {}", p.hr.map_or("—".into(), |h| format!("{h:.0} bpm"))));
        });
        // Elevation profile with a cursor marker.
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 50.0), egui::Sense::hover());
        let eles: Vec<f64> = leg.points.iter().filter_map(|p| p.ele).collect();
        if eles.len() > 1 {
            let (lo, hi) = eles.iter().fold((f64::MAX, f64::MIN), |(a, b), e| (a.min(*e), b.max(*e)));
            let d0 = leg.points[0].dist;
            let span = run.max(1.0) as f32;
            let pt = |d: f64, e: f64| {
                Pos2::new(
                    rect.left() + rect.width() * (d - d0) as f32 / span,
                    rect.bottom() - rect.height() * ((e - lo) / (hi - lo).max(1.0)) as f32,
                )
            };
            let line: Vec<Pos2> = leg.points.iter().filter_map(|p| p.ele.map(|e| pt(p.dist, e))).collect();
            ui.painter().add(egui::Shape::line(line, Stroke::new(1.5, Color32::GRAY)));
            let x = rect.left() + rect.width() * self.cursor_m as f32 / span;
            ui.painter().line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0, Color32::RED));
        }
    }

    /// Places the selected point where the map is clicked, and draws the placed points and the route.
    fn calibration_overlay(&mut self, resp: &egui::Response, painter: &egui::Painter, rect: Rect) {
        let csv_to_page = self.csv_to_page as f64;
        let (offset, zoom, page_size) = (self.offset, self.zoom, self.page_size);
        let Some(cal) = self.calib.as_mut() else { return };
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
            let mut from = r.start_idx;
            for &(slot, to) in &r.visits {
                let line: Vec<Pos2> = r.track[from..=to].iter().map(|p| screen(p.page)).collect();
                painter.add(egui::Shape::line(line, Stroke::new(3.0, Self::leg_color(slot - 1))));
                from = to;
            }
        }
        let magenta = Color32::from_rgb(200, 0, 200);
        let placed: Vec<Pos2> = cal.slots.iter().flatten().map(|c| screen(*c)).collect();
        painter.add(egui::Shape::line(placed, Stroke::new(1.0, magenta.gamma_multiply(0.6))));
        for (k, c) in cal.slots.iter().enumerate() {
            let Some(c) = c else { continue };
            let p = screen(*c);
            let radius = if cal.selected == Some(k) { 6.0 } else { 4.0 };
            painter.circle_filled(p, radius, magenta);
            painter.text(p + Vec2::new(8.0, -8.0), egui::Align2::LEFT_BOTTOM, cal.slot_tag(k), egui::FontId::proportional(16.0), Color32::from_rgb(120, 0, 160));
        }
    }

    fn map_view(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = resp.rect;
        if !self.fitted {
            self.fit(rect);
        }
        if let Some(leg) = self.pending_fit.take() {
            self.fit_leg(rect, leg);
        }
        if resp.dragged() {
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
            painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }

        if self.calibrating {
            self.calibration_overlay(&resp, &painter, rect);
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
            let width = if selected { 3.5 } else { 2.0 };
            let faded = |c: Color32| if selected { c } else { c.gamma_multiply(0.3) };
            let pts: Vec<(&Point, Pos2)> = leg
                .points
                .iter()
                .filter_map(|p| p.px.map(|px| (p, self.to_screen(rect, px))))
                .collect();
            if self.mode == ColorMode::Leg || !selected {
                let line: Vec<Pos2> = pts.iter().map(|(_, s)| *s).collect();
                if !drawn_apart {
                    painter.add(egui::Shape::line(line, Stroke::new(width, faded(Self::leg_color(i)))));
                }
            } else if !drawn_apart {
                for w in pts.windows(2) {
                    painter.line_segment([w[0].1, w[1].1], Stroke::new(width + 0.5, self.value_color(w[1].0)));
                }
            }
            if let Some((_, end)) = pts.last() {
                painter.circle_filled(*end, 6.0, Color32::WHITE);
                painter.circle_filled(*end, 4.5, faded(Self::leg_color(i)));
            }
        }

        if let Some(cal) = &self.calib {
            if let (Some(t), Some(report)) = (cal.placing, &cal.report) {
                let pts = report.before_first_control();
                let screen = |p: (f64, f64)| rect.min + self.offset + Vec2::new(p.0 as f32, p.1 as f32) * self.zoom;
                // The whole track before control 1, faint; leg 1 from the chosen time, strong.
                let all: Vec<Pos2> = pts.iter().map(|p| screen(p.page)).collect();
                painter.add(egui::Shape::line(all, Stroke::new(1.5, Color32::from_gray(150))));
                let k = pts.partition_point(|p| p.t <= t).saturating_sub(1).min(pts.len() - 1);
                let mut at = pts[k].page;
                if let Some(next) = pts.get(k + 1).filter(|n| n.t > pts[k].t) {
                    let f = ((t - pts[k].t) / (next.t - pts[k].t)).clamp(0.0, 1.0);
                    at = (at.0 + (next.page.0 - at.0) * f, at.1 + (next.page.1 - at.1) * f);
                }
                let mut line = vec![screen(at)];
                line.extend(pts[k + 1..].iter().map(|p| screen(p.page)));
                painter.add(egui::Shape::line(line, Stroke::new(3.5, Self::leg_color(0))));
                if let Some(c) = cal.slots[0] {
                    let p = screen(c);
                    painter.circle_filled(p, 4.0, Color32::from_rgb(200, 0, 200));
                    painter.text(p + Vec2::new(8.0, -8.0), egui::Align2::LEFT_BOTTOM, "S", egui::FontId::proportional(16.0), Color32::from_rgb(120, 0, 160));
                }
                let c = screen(at);
                painter.circle_filled(c, 7.0, Color32::BLACK);
                painter.circle_filled(c, 5.0, Color32::from_rgb(255, 230, 0));
                return;
            }
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
        egui::Panel::left("side").resizable(false).show(ui, |ui| self.side_panel(ui));
        if !self.calibrating {
            egui::Panel::bottom("bottom").show(ui, |ui| self.bottom_panel(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| self.map_view(ui));
        self.placement_window(&ui.ctx().clone());
    }
}

fn fmt_dur(s: f64) -> String {
    let s = s.round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn fmt_pace(s_per_km: f64) -> String {
    let s = s_per_km.round() as i64;
    format!("{}:{:02} /km", s / 60, s % 60)
}
