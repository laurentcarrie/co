//! Calibration: place a GPS track on the scanned map from the controls clicked by the user.
//!
//! The user clicks start, controls 1..n, finish, in order, on the map. We then look for the
//! similarity transform (scale, rotation, shift) from GPS metres to map pixels, together with
//! the moment the runner visited each control, minimising the distance between the visited
//! track points and the clicks. Finally the small GPS drift is removed by interpolating the
//! residual offset between consecutive controls. Output is the CSV read by the viewer.

use crate::tcx::Sample;
use anyhow::{Context, Result, bail};
use std::path::Path;

type V = (f64, f64);

fn d2(a: V, b: V) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

/// Similarity transform `w = a * z + b`, with points seen as complex numbers.
#[derive(Clone, Copy, Debug)]
struct Sim {
    a: V,
    b: V,
}

impl Sim {
    fn apply(&self, z: V) -> V {
        (self.a.0 * z.0 - self.a.1 * z.1 + self.b.0, self.a.0 * z.1 + self.a.1 * z.0 + self.b.1)
    }

    fn scale(&self) -> f64 {
        self.a.0.hypot(self.a.1)
    }
}

/// Least-squares similarity fit of `pairs` (z -> w).
fn fit_sim(pairs: &[(V, V)]) -> Option<Sim> {
    let n = pairs.len() as f64;
    let zc = (pairs.iter().map(|p| p.0.0).sum::<f64>() / n, pairs.iter().map(|p| p.0.1).sum::<f64>() / n);
    let wc = (pairs.iter().map(|p| p.1.0).sum::<f64>() / n, pairs.iter().map(|p| p.1.1).sum::<f64>() / n);
    let (mut re, mut im, mut den) = (0.0, 0.0, 0.0);
    for (z, w) in pairs {
        let dz = (z.0 - zc.0, z.1 - zc.1);
        let dw = (w.0 - wc.0, w.1 - wc.1);
        re += dz.0 * dw.0 + dz.1 * dw.1;
        im += dz.0 * dw.1 - dz.1 * dw.0;
        den += dz.0 * dz.0 + dz.1 * dz.1;
    }
    if den < 1e-9 {
        return None;
    }
    let a = (re / den, im / den);
    let b = (wc.0 - (a.0 * zc.0 - a.1 * zc.1), wc.1 - (a.0 * zc.1 + a.1 * zc.0));
    Some(Sim { a, b })
}

/// Visit index (into `pts`) of each click, strictly increasing, minimising the squared distances.
fn assign(sim: &Sim, pts: &[V], clicks: &[V]) -> Vec<usize> {
    let (n, k) = (pts.len(), clicks.len());
    let proj: Vec<V> = pts.iter().map(|p| sim.apply(*p)).collect();
    let mut prev: Vec<f64> = proj.iter().map(|p| d2(*p, clicks[0])).collect();
    let mut back: Vec<Vec<u32>> = Vec::with_capacity(k);
    for c in 1..k {
        let mut cur = vec![f64::INFINITY; n];
        let mut bk = vec![0u32; n];
        let (mut best, mut best_i) = (f64::INFINITY, 0usize);
        for i in 1..n {
            if prev[i - 1] < best {
                best = prev[i - 1];
                best_i = i - 1;
            }
            cur[i] = d2(proj[i], clicks[c]) + best;
            bk[i] = best_i as u32;
        }
        back.push(bk);
        prev = cur;
    }
    let mut i = (0..n).min_by(|&a, &b| prev[a].total_cmp(&prev[b])).unwrap_or(0);
    let mut idx = vec![0; k];
    idx[k - 1] = i;
    for c in (1..k).rev() {
        i = back[c - 1][i] as usize;
        idx[c - 1] = i;
    }
    idx
}

struct Solution {
    sim: Sim,
    visit: Vec<usize>,
    rms: f64,
}

fn refine(init: Sim, pts: &[V], clicks: &[V]) -> Option<Solution> {
    let pairs_of = |visit: &[usize]| -> Vec<(V, V)> { visit.iter().zip(clicks).map(|(&i, &c)| (pts[i], c)).collect() };
    let mut sim = init;
    let mut visit = assign(&sim, pts, clicks);
    for _ in 0..40 {
        sim = fit_sim(&pairs_of(&visit))?;
        let next = assign(&sim, pts, clicks);
        if next == visit {
            break;
        }
        visit = next;
    }
    let rms = (visit.iter().zip(clicks).map(|(&i, &c)| d2(sim.apply(pts[i]), c)).sum::<f64>() / clicks.len() as f64).sqrt();
    Some(Solution { sim, visit, rms })
}

fn bbox(v: &[V]) -> (V, V) {
    let mut lo = v[0];
    let mut hi = v[0];
    for p in v {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    }
    (lo, hi)
}

fn solve(pts: &[V], clicks: &[V]) -> Result<Solution> {
    // Nothing ties the track ends to the clicks (the watch may run before the start and after the
    // finish), so start from many guesses: the map is roughly north-up, and the scale is about the
    // ratio of the extents of the clicks and of the track. Several small rotations and scale errors
    // are tried; the DP + least-squares loop converges from the right one, the best fit wins.
    let ((tl, th), (cl, ch)) = (bbox(pts), bbox(clicks));
    let (td, cd) = ((th.0 - tl.0).hypot(th.1 - tl.1), (ch.0 - cl.0).hypot(ch.1 - cl.1));
    let mut inits = Vec::new();
    if td > 1.0 && cd > 1.0 {
        let tc = ((tl.0 + th.0) / 2.0, (tl.1 + th.1) / 2.0);
        let cc = ((cl.0 + ch.0) / 2.0, (cl.1 + ch.1) / 2.0);
        for mul in [0.8, 1.0, 1.25] {
            for deg in (-20..=20).step_by(5) {
                let s = cd / td * mul;
                let (sin, cos) = (deg as f64).to_radians().sin_cos();
                let a = (s * cos, s * sin);
                // Match the centres of the extents.
                let rot = (a.0 * tc.0 - a.1 * tc.1, a.0 * tc.1 + a.1 * tc.0);
                inits.push(Sim { a, b: (cc.0 - rot.0, cc.1 - rot.1) });
            }
        }
    }
    inits
        .into_iter()
        .filter_map(|i| refine(i, pts, clicks))
        .min_by(|a, b| a.rms.total_cmp(&b.rms))
        .context("could not fit the track to the clicked controls")
}

/// A GPS point of the track, placed on the map.
pub struct TrackPoint {
    /// Index into the TCX samples.
    pub sample: usize,
    /// Seconds since the first sample.
    pub t: f64,
    /// Fitted position, in page pixels.
    pub page: V,
}

pub struct Report {
    pub scale_px_per_m: f64,
    /// (label, error in metres) for the placed controls and the finish.
    pub errors_m: Vec<(String, f64)>,
    pub mean_err_m: f64,
    pub max_err_m: f64,
    /// Distance between the clicked start triangle and the track point chosen as the start of leg 1.
    pub start_gap_m: f64,
    pub rows: usize,
    /// Every GPS point of the track, fitted, in page pixels.
    pub track: Vec<TrackPoint>,
    /// Index into `track` of the start of leg 1.
    pub start_idx: usize,
    /// For every placed control and the finish: (slot, index into `track` of its visit). Slot 1 is
    /// control 1, the last slot is the finish.
    pub visits: Vec<(usize, usize)>,
}

impl Report {
    /// The track from the watch start to the first placed control: when the triangle was passed is
    /// somewhere in there.
    pub fn before_first_control(&self) -> &[TrackPoint] {
        &self.track[..=self.visits[0].1]
    }
}

/// Label of control `j` (0-based among the controls and the finish).
fn label(j: usize, n_ctrl: usize) -> String {
    if j + 1 == n_ctrl { "F".into() } else { (j + 1).to_string() }
}

/// Fits the track to `slots` (page pixels: start triangle, controls, finish; `None` for a point
/// not placed yet). The triangle and the finish are required, the controls are optional.
///
/// With `out`, also writes the CSV (CSV pixel coordinates are page pixels divided by `csv_to_page`);
/// then every point must be placed. `start_sample` is the sample where the triangle was passed,
/// when the user said so: leg 1 starts there instead of at the track point nearest to the triangle.
pub fn calibrate(
    samples: &[Sample],
    slots: &[Option<V>],
    csv_to_page: f64,
    start_sample: Option<usize>,
    out: Option<&Path>,
) -> Result<Report> {
    let n_leg = slots.len().saturating_sub(1); // controls and finish
    if n_leg < 1 || slots[0].is_none() || slots[n_leg].is_none() {
        bail!("place the start triangle and the finish");
    }
    if out.is_some() && slots.iter().any(Option::is_none) {
        bail!("place every control first");
    }
    let placed: Vec<(usize, V)> = slots.iter().enumerate().filter_map(|(k, c)| c.map(|c| (k, c))).collect();
    let clicks: Vec<V> = placed.iter().map(|p| p.1).collect();
    let gps: Vec<usize> = (0..samples.len()).filter(|&i| samples[i].lat_lon.is_some()).collect();
    if gps.len() < clicks.len() + 1 {
        bail!("the track has fewer GPS points than clicks");
    }
    let (lat0, lon0) = (
        gps.iter().map(|&i| samples[i].lat_lon.unwrap().0).sum::<f64>() / gps.len() as f64,
        gps.iter().map(|&i| samples[i].lat_lon.unwrap().1).sum::<f64>() / gps.len() as f64,
    );
    let r = 6_371_000.0;
    // Metres east / south of the track centre: south so that "down" matches image rows.
    let pts: Vec<V> = gps
        .iter()
        .map(|&i| {
            let (la, lo) = samples[i].lat_lon.unwrap();
            ((lo - lon0).to_radians() * lat0.to_radians().cos() * r, -(la - lat0).to_radians() * r)
        })
        .collect();

    // The start triangle takes part in the fit like the controls: it is only a position, the moment
    // the runner was there is searched with the others (and refined later by the user).
    let sol = solve(&pts, &clicks)?;
    let sim = sol.sim;
    let scale = sim.scale();
    let (start, start_visit) = (clicks[0], sol.visit[0]);
    let (ctrl, visit) = (&clicks[1..], &sol.visit[1..]);
    // Leg 1 begins where the user said, else at the visit of the triangle.
    let start_j = start_sample
        .map(|s| gps.partition_point(|&i| i <= s).saturating_sub(1).min(visit[0].saturating_sub(1)))
        .unwrap_or(start_visit);

    // Constant offsets at each clicked control and the finish, interpolated in between to cancel GPS
    // drift.
    let off: Vec<V> = visit
        .iter()
        .zip(ctrl)
        .map(|(&i, &c)| {
            let p = sim.apply(pts[i]);
            (c.0 - p.0, c.1 - p.1)
        })
        .collect();
    let offset_at = |i: usize| -> V {
        if i <= visit[0] {
            return off[0];
        }
        if i >= visit[visit.len() - 1] {
            return off[off.len() - 1];
        }
        let c = visit.partition_point(|&x| x <= i) - 1;
        let f = (i - visit[c]) as f64 / (visit[c + 1] - visit[c]) as f64;
        (off[c].0 + (off[c + 1].0 - off[c].0) * f, off[c].1 + (off[c + 1].1 - off[c].1) * f)
    };
    let page_of = |j: usize| -> V {
        let (p, o) = (sim.apply(pts[j]), offset_at(j));
        (p.0 + o.0, p.1 + o.1)
    };

    if let Some(out) = out {
        // Leg of every sample: leg j covers visit[j-1]+1 ..= visit[j]; leg 0 starts at the triangle.
        let visit_sample: Vec<usize> = visit.iter().map(|&j| gps[j]).collect();
        let mut leg_of: Vec<Option<usize>> = vec![None; samples.len()];
        for j in 0..n_leg {
            let lo = if j == 0 { gps[start_j] } else { visit_sample[j - 1] + 1 };
            for s in lo..=visit_sample[j] {
                leg_of[s] = Some(j);
            }
        }
        let gps_pos: std::collections::HashMap<usize, usize> = gps.iter().enumerate().map(|(j, &s)| (s, j)).collect();

        let mut w = csv::Writer::from_path(out).with_context(|| format!("cannot write {}", out.display()))?;
        w.write_record([
            "time_utc", "elapsed_s", "to_control", "lat", "lon", "elevation_m", "heart_rate", "speed_ms",
            "pace_min_km", "distance_m", "x_px", "y_px", "x_px_fit", "y_px_fit",
        ])?;
        let f = |v: Option<f64>| v.map_or(String::new(), |x| x.to_string());
        let px = |x: f64| format!("{:.1}", x / csv_to_page);
        for (s, smp) in samples.iter().enumerate() {
            let (mut x, mut y, mut xf, mut yf) = (String::new(), String::new(), String::new(), String::new());
            if let Some(&j) = gps_pos.get(&s) {
                let p = sim.apply(pts[j]);
                let fit = page_of(j);
                (x, y, xf, yf) = (px(p.0), px(p.1), px(fit.0), px(fit.1));
            }
            let leg = leg_of[s].map_or(String::new(), |j| label(j, n_leg));
            let (la, lo) = smp.lat_lon.unzip();
            let pace = (smp.speed > 0.3).then(|| 1000.0 / smp.speed / 60.0);
            w.write_record([
                smp.time_utc.clone(), format!("{:.0}", smp.t), leg, f(la), f(lo), f(smp.ele), f(smp.hr),
                format!("{:.2}", smp.speed), f(pace), format!("{:.1}", smp.dist), x, y, xf, yf,
            ])?;
        }
        w.flush()?;
    }

    let errors_m: Vec<(String, f64)> = visit
        .iter()
        .zip(ctrl)
        .zip(&placed[1..])
        .map(|((&i, &cl), &(slot, _))| (label(slot - 1, n_leg), d2(sim.apply(pts[i]), cl).sqrt() / scale))
        .collect();
    let mean_err_m = errors_m.iter().map(|e| e.1).sum::<f64>() / errors_m.len() as f64;
    let max_err_m = errors_m.iter().map(|e| e.1).fold(0.0, f64::max);
    let start_gap_m = d2(sim.apply(pts[start_j]), start).sqrt() / scale;
    Ok(Report {
        scale_px_per_m: scale,
        errors_m,
        mean_err_m,
        max_err_m,
        start_gap_m,
        rows: samples.len(),
        track: (0..gps.len()).map(|j| TrackPoint { sample: gps[j], t: samples[gps[j]].t, page: page_of(j) }).collect(),
        start_idx: start_j,
        visits: placed[1..].iter().zip(visit).map(|(&(slot, _), &j)| (slot, j)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L-shaped track (east, then north), one sample per second, 7.86 m per sample; the map is
    /// at 0.2 px/m. Triangle at sample 20, controls at 40, 50 (the corner), 75, finish at 95.
    fn l_track() -> (Vec<Sample>, Vec<V>) {
        let step_m = 7.86;
        let (dlon, dlat) = (1e-4, step_m / 111_195.0);
        let samples: Vec<Sample> = (0..100)
            .map(|i| {
                let (e, n) = (i.min(50) as f64, (i.max(50) - 50) as f64);
                Sample {
                    time_utc: format!("t{i}"),
                    t: i as f64,
                    lat_lon: Some((45.0 + n * dlat, 5.0 + e * dlon)),
                    ele: None,
                    hr: None,
                    dist: i as f64 * step_m,
                    speed: 3.0,
                }
            })
            .collect();
        let px = |i: usize| {
            let (e, n) = (i.min(50) as f64, (i.max(50) - 50) as f64);
            (e * step_m * 0.2, 500.0 - n * step_m * 0.2)
        };
        (samples, [20, 40, 50, 75, 95].into_iter().map(px).collect())
    }

    #[test]
    fn start_sample_sets_start_of_leg_1() {
        let (samples, clicks) = l_track();
        let clicks: Vec<Option<V>> = clicks.into_iter().map(Some).collect();
        let out = std::env::temp_dir().join("vcc-start-test.csv");
        let auto = calibrate(&samples, &clicks, 1.0, None, Some(&out)).unwrap();
        let before = auto.before_first_control();
        assert!(before.len() > 2);
        assert_eq!(before[0].sample, 0);
        assert!(before.last().unwrap().t > 10.0);
        let chosen = calibrate(&samples, &clicks, 1.0, Some(10), Some(&out)).unwrap();
        assert_eq!(chosen.track[chosen.start_idx].sample, 10);
        // The fit does not depend on the start of leg 1.
        assert!((chosen.mean_err_m - auto.mean_err_m).abs() < 1e-9);
        let csv = std::fs::read_to_string(&out).unwrap();
        let first = csv.lines().skip(1).position(|l| l.split(',').nth(2) == Some("1")).unwrap();
        assert_eq!(first, 10);
    }

    #[test]
    fn fits_with_missing_controls() {
        let (samples, clicks) = l_track();
        let mut slots: Vec<Option<V>> = clicks.into_iter().map(Some).collect();
        slots[2] = None;
        slots[3] = None;
        let r = calibrate(&samples, &slots, 1.0, None, None).unwrap();
        assert_eq!(r.visits.iter().map(|v| v.0).collect::<Vec<_>>(), [1, 4]);
        // Only the triangle and the finish: a (rough) route is still drawn.
        slots[1] = None;
        assert!(calibrate(&samples, &slots, 1.0, None, None).is_ok());
        // Writing the CSV needs every point.
        assert!(calibrate(&samples, &slots, 1.0, None, Some(&std::env::temp_dir().join("vcc-x.csv"))).is_err());
        // The triangle and the finish are required.
        slots[0] = None;
        assert!(calibrate(&samples, &slots, 1.0, None, None).is_err());
    }

    /// Local check against a real race. Run with:
    /// VCC_TCX=... VCC_CLICKS="x,y;x,y;..." cargo test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_race() {
        let tcx = std::env::var("VCC_TCX").unwrap();
        let clicks: Vec<Option<V>> = std::env::var("VCC_CLICKS")
            .unwrap()
            .split(';')
            .map(|p| {
                let (x, y) = p.split_once(',').unwrap();
                Some((x.parse().unwrap(), y.parse().unwrap()))
            })
            .collect();
        let samples = crate::tcx::load(Path::new(&tcx)).unwrap();
        let out = std::env::temp_dir().join("vcc-test.csv");
        let r = calibrate(&samples, &clicks, 1.0, None, Some(&out)).unwrap();
        println!("scale {:.3} px/m, mean {:.1} m, max {:.1} m", r.scale_px_per_m, r.mean_err_m, r.max_err_m);
        for (l, e) in &r.errors_m {
            println!("  {l}: {e:.1} m");
        }
        println!("csv: {}", out.display());
    }
}
