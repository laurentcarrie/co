//! Track data loaded from the CSV: points grouped into legs, with per-leg stats.

use anyhow::{Context, Result};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Point {
    pub t: f64,
    pub dist: f64,
    pub ele: Option<f64>,
    pub hr: Option<f64>,
    /// Speed in m/s, smoothed.
    pub speed: f64,
    pub lat_lon: Option<(f64, f64)>,
    /// Position on the map page, in pixels at the CSV resolution.
    pub px: Option<(f32, f32)>,
}

impl Point {
    /// Pace in seconds per km, or `None` when (almost) standing still.
    pub fn pace_s_km(&self) -> Option<f64> {
        (self.speed > 0.3).then(|| 1000.0 / self.speed)
    }
}

#[derive(Debug)]
pub struct Leg {
    /// Label of the target control: "1", "2", ..., "F".
    pub name: String,
    pub points: Vec<Point>,
}

#[derive(Debug)]
pub struct LegStats {
    pub duration_s: f64,
    pub run_m: f64,
    pub bird_m: Option<f64>,
}

impl Leg {
    /// Length of the run in this leg, in metres.
    pub fn run_m(&self) -> f64 {
        self.points[self.points.len() - 1].dist - self.points[0].dist
    }

    /// Index of the last trackpoint reached after `d` metres from the leg start.
    pub fn index_at(&self, d: f64) -> usize {
        let d0 = self.points[0].dist;
        self.points.partition_point(|p| p.dist - d0 <= d).saturating_sub(1)
    }

    /// Map position after `d` metres from the leg start, interpolated between trackpoints.
    pub fn px_at(&self, d: f64) -> Option<(f32, f32)> {
        let k = self.index_at(d);
        let a = &self.points[k];
        if let (Some(pa), Some(b)) = (a.px, self.points.get(k + 1)) {
            if let Some(pb) = b.px.filter(|_| b.dist > a.dist) {
                let f = ((d + self.points[0].dist - a.dist) / (b.dist - a.dist)).clamp(0.0, 1.0) as f32;
                return Some((pa.0 + (pb.0 - pa.0) * f, pa.1 + (pb.1 - pa.1) * f));
            }
        }
        // No position here: use the closest earlier one.
        self.points[..=k].iter().rev().find_map(|p| p.px)
    }

    pub fn stats(&self) -> LegStats {
        let first = &self.points[0];
        let last = &self.points[self.points.len() - 1];
        let bird_m = match (first_gps(&self.points), last_gps(&self.points)) {
            (Some(a), Some(b)) => Some(haversine_m(a, b)),
            _ => None,
        };
        LegStats { duration_s: last.t - first.t, run_m: last.dist - first.dist, bird_m }
    }
}

fn first_gps(p: &[Point]) -> Option<(f64, f64)> {
    p.iter().find_map(|p| p.lat_lon)
}

fn last_gps(p: &[Point]) -> Option<(f64, f64)> {
    p.iter().rev().find_map(|p| p.lat_lon)
}

fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6_371_000.0;
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = la2 - la1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

pub struct Course {
    pub legs: Vec<Leg>,
}

fn num(rec: &csv::StringRecord, i: Option<usize>) -> Option<f64> {
    rec.get(i?)?.trim().parse().ok()
}

/// Moving average of `v` over +-`w` samples, ignoring missing values.
fn smooth(v: &[Option<f64>], w: usize) -> Vec<f64> {
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(w), (i + w + 1).min(v.len()));
            let xs: Vec<f64> = v[a..b].iter().flatten().copied().collect();
            if xs.is_empty() { 0.0 } else { xs.iter().sum::<f64>() / xs.len() as f64 }
        })
        .collect()
}

impl Course {
    /// A course without any leg (used while calibrating).
    pub fn empty() -> Self {
        Course { legs: Vec::new() }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut rdr = csv::Reader::from_path(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        let headers = rdr.headers()?.clone();
        let col = |name: &str| headers.iter().position(|h| h == name);
        let need = |name: &str| col(name).with_context(|| format!("missing CSV column `{name}`"));

        let (c_t, c_leg, c_dist) = (need("elapsed_s")?, need("to_control")?, need("distance_m")?);
        let (c_lat, c_lon) = (col("lat"), col("lon"));
        let (c_ele, c_hr, c_speed) = (col("elevation_m"), col("heart_rate"), col("speed_ms"));
        // Prefer the fitted pixel coordinates when the CSV has them.
        let (c_x, c_y) = match (col("x_px_fit"), col("y_px_fit")) {
            (Some(x), Some(y)) => (Some(x), Some(y)),
            _ => (col("x_px"), col("y_px")),
        };

        let mut names: Vec<String> = Vec::new();
        let mut groups: Vec<Vec<Point>> = Vec::new();
        let mut raw_speed: Vec<Vec<Option<f64>>> = Vec::new();
        for rec in rdr.records() {
            let rec = rec?;
            let name = rec.get(c_leg).unwrap_or("").trim().to_string();
            if name.is_empty() {
                continue; // before the start or after the finish
            }
            if names.last() != Some(&name) {
                names.push(name);
                groups.push(Vec::new());
                raw_speed.push(Vec::new());
            }
            let px = match (num(&rec, c_x), num(&rec, c_y)) {
                (Some(x), Some(y)) => Some((x as f32, y as f32)),
                _ => None,
            };
            let lat_lon = match (num(&rec, c_lat), num(&rec, c_lon)) {
                (Some(a), Some(b)) => Some((a, b)),
                _ => None,
            };
            raw_speed.last_mut().unwrap().push(num(&rec, c_speed));
            groups.last_mut().unwrap().push(Point {
                t: num(&rec, Some(c_t)).unwrap_or(0.0),
                dist: num(&rec, Some(c_dist)).unwrap_or(0.0),
                ele: num(&rec, c_ele),
                hr: num(&rec, c_hr),
                speed: 0.0,
                lat_lon,
                px,
            });
        }

        let mut legs = Vec::new();
        for ((name, mut points), sp) in names.into_iter().zip(groups).zip(raw_speed) {
            for (p, s) in points.iter_mut().zip(smooth(&sp, 5)) {
                p.speed = s;
            }
            legs.push(Leg { name, points });
        }
        anyhow::ensure!(!legs.is_empty(), "no data rows in {}", path.display());
        Ok(Course { legs })
    }

    /// 5th and 95th percentile of an attribute over all points, for colour scales.
    pub fn range(&self, f: impl Fn(&Point) -> Option<f64>) -> (f64, f64) {
        let mut v: Vec<f64> = self.legs.iter().flat_map(|l| l.points.iter()).filter_map(&f).collect();
        if v.is_empty() {
            return (0.0, 1.0);
        }
        v.sort_by(|a, b| a.total_cmp(b));
        (v[v.len() * 5 / 100], v[(v.len() * 95 / 100).min(v.len() - 1)])
    }
}
