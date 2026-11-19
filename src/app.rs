use crate::calib;
use crate::model::{Course, Leg, Point};
use crate::pdf::PageImage;
use crate::tcx::Sample;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use std::path::PathBuf;

/// Data needed to (re)calibrate a track on the map.
pub struct CalibSession {
    pub samples: Vec<Sample>,
    pub out: PathBuf,
    /// Clicked controls in page pixels: start, 1..n, finish.
    clicks: Vec<(f64, f64)>,
    message: String,
    report: Option<calib::Report>,
    /// The lap button was pressed at the triangle: the first watch lap ends there.
    lap_at_triangle: bool,
}

impl CalibSession {
    pub fn new(samples: Vec<Sample>, out: PathBuf) -> Self {
        CalibSession { samples, out, clicks: Vec::new(), message: String::new(), report: None, lap_at_triangle: false }
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

    /// Fits the track to the clicked controls, writes the CSV and switches to the viewer.
    fn run_calibration(&mut self) {
        let Some(cal) = self.calib.as_mut() else { return };
        let csv_to_page = self.csv_to_page as f64;
        match calib::calibrate(&cal.samples, &cal.clicks, csv_to_page, cal.lap_at_triangle, &cal.out)
            .and_then(|r| Course::load(&cal.out).map(|c| (r, c)))
        {
            Ok((report, course)) => {
                cal.message = format!("Saved {}", cal.out.display());
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
        let Some(cal) = self.calib.as_mut() else { return };
        ui.heading("Calibration");
        ui.label("Click on the map, in this order: the start triangle (it only marks where leg 1 begins), control 1, 2, ... and the finish. Then press Compute.");
        let n = cal.clicks.len();
        let next = if n == 0 { "start triangle".to_string() } else { format!("control {n} (or the finish)") };
        ui.label(egui::RichText::new(format!("Next click: {next}")).strong());
        ui.separator();
        ui.label(format!("{n} points placed"));
        ui.horizontal(|ui| {
            if ui.add_enabled(n > 0, egui::Button::new("Undo")).clicked() {
                cal.clicks.pop();
            }
            if ui.add_enabled(n > 0, egui::Button::new("Reset")).clicked() {
                cal.clicks.clear();
            }
        });
        let laps = cal.samples.iter().map(|s| s.lap).max().map_or(0, |m| m + 1);
        ui.add_enabled_ui(laps >= 2, |ui| {
            ui.checkbox(&mut cal.lap_at_triangle, "I pressed lap at the triangle")
                .on_hover_text("The first watch lap then ends at the triangle: its start is not on the map and is ignored.");
        });
        if laps < 2 {
            ui.label("(this TCX has a single lap)");
        }
        let compute = ui.add_enabled(n >= 3, egui::Button::new("Compute (last point = finish)"));
        if !cal.message.is_empty() {
            ui.label(&cal.message);
        }
        if compute.clicked() {
            self.run_calibration();
        }
        if self.course.legs.len() > 0 && ui.button("Back to viewer").clicked() {
            self.calibrating = false;
        }
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
            if resp.clicked() {
                if let (Some(pos), Some(cal)) = (resp.interact_pointer_pos(), self.calib.as_mut()) {
                    let p = (pos - rect.min - self.offset) / self.zoom;
                    if p.x >= 0.0 && p.y >= 0.0 && p.x <= self.page_size.x && p.y <= self.page_size.y {
                        cal.clicks.push((p.x as f64, p.y as f64));
                        cal.message.clear();
                    }
                }
            }
            if let Some(cal) = &self.calib {
                let screen = |c: &(f64, f64)| rect.min + self.offset + Vec2::new(c.0 as f32, c.1 as f32) * self.zoom;
                let line: Vec<Pos2> = cal.clicks.iter().map(screen).collect();
                painter.add(egui::Shape::line(line, Stroke::new(1.0, Color32::from_rgb(200, 0, 200))));
                for (k, c) in cal.clicks.iter().enumerate() {
                    let p = screen(c);
                    painter.circle_filled(p, 4.0, Color32::from_rgb(200, 0, 200));
                    let text = if k == 0 { "S".to_string() } else { k.to_string() };
                    painter.text(p + Vec2::new(8.0, -8.0), egui::Align2::LEFT_BOTTOM, text, egui::FontId::proportional(16.0), Color32::from_rgb(120, 0, 160));
                }
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
            let width = if selected { 3.5 } else { 2.0 };
            let faded = |c: Color32| if selected { c } else { c.gamma_multiply(0.3) };
            let pts: Vec<(&Point, Pos2)> = leg
                .points
                .iter()
                .filter_map(|p| p.px.map(|px| (p, self.to_screen(rect, px))))
                .collect();
            if self.mode == ColorMode::Leg || !selected {
                let line: Vec<Pos2> = pts.iter().map(|(_, s)| *s).collect();
                painter.add(egui::Shape::line(line, Stroke::new(width, faded(Self::leg_color(i)))));
            } else {
                for w in pts.windows(2) {
                    painter.line_segment([w[0].1, w[1].1], Stroke::new(width + 0.5, self.value_color(w[1].0)));
                }
            }
            if let Some((_, end)) = pts.last() {
                painter.circle_filled(*end, 6.0, Color32::WHITE);
                painter.circle_filled(*end, 4.5, faded(Self::leg_color(i)));
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
