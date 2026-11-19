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

fn refine(init: Sim, pts: &[V], clicks: &[V], anchor: Option<(V, V)>) -> Option<Solution> {
    let pairs_of = |visit: &[usize]| {
        let mut p: Vec<(V, V)> = visit.iter().zip(clicks).map(|(&i, &c)| (pts[i], c)).collect();
        p.extend(anchor);
        p
    };
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

/// `anchor` is an optional fixed (track point, click) pair, known in advance (not searched).
fn solve(pts: &[V], clicks: &[V], anchor: Option<(V, V)>) -> Result<Solution> {
    // Nothing ties the track ends to the clicks (the watch may run before the start and after the
    // finish), so start from many guesses: the map is roughly north-up, and the scale is about the
    // ratio of the extents of the clicks and of the track. Several small rotations and scale errors
    // are tried; the DP + least-squares loop converges from the right one, the best fit wins.
    let mut all_clicks = clicks.to_vec();
    all_clicks.extend(anchor.map(|a| a.1));
    let ((tl, th), (cl, ch)) = (bbox(pts), bbox(&all_clicks));
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
                // Match the anchor exactly when there is one, else the centres of the extents.
                let (z, w) = anchor.unwrap_or((tc, cc));
                let rot = (a.0 * z.0 - a.1 * z.1, a.0 * z.1 + a.1 * z.0);
                inits.push(Sim { a, b: (w.0 - rot.0, w.1 - rot.1) });
            }
        }
    }
    inits
        .into_iter()
        .filter_map(|i| refine(i, pts, clicks, anchor))
        .min_by(|a, b| a.rms.total_cmp(&b.rms))
        .context("could not fit the track to the clicked controls")
}

pub struct Report {
    pub scale_px_per_m: f64,
    /// (label, error in metres) for the controls and the finish.
    pub errors_m: Vec<(String, f64)>,
    pub mean_err_m: f64,
    pub max_err_m: f64,
    /// Distance between the clicked start triangle and the track point chosen as the start of leg 1:
    /// the end of lap 1 if the lap was pressed at the triangle, else the nearest point before control 1.
    pub start_gap_m: f64,
    pub rows: usize,
}

/// Label of control `j` (0-based among the controls and the finish).
fn label(j: usize, n_ctrl: usize) -> String {
    if j + 1 == n_ctrl { "F".into() } else { (j + 1).to_string() }
}

/// Fits the track to `clicks` (page pixels: start triangle, controls, finish) and writes the CSV.
/// CSV pixel coordinates are page pixels divided by `csv_to_page`.
pub fn calibrate(samples: &[Sample], clicks: &[V], csv_to_page: f64, lap_at_triangle: bool, out: &Path) -> Result<Report> {
    if clicks.len() < 3 {
        bail!("click the start triangle, at least one control and the finish");
    }
    let (start, ctrl) = (clicks[0], &clicks[1..]);
    let gps: Vec<usize> = (0..samples.len()).filter(|&i| samples[i].lat_lon.is_some()).collect();
    if gps.len() < ctrl.len() + 1 {
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

    // Race part of the track. If the lap button was pressed at the triangle, the first watch lap
    // (watch start -> triangle) ends exactly there: the triangle is then a real anchor, visited at
    // the end of lap 1, and everything before it (whose start is not on the map) is ignored.
    // Otherwise the triangle is only a marker for the start of leg 1 and is not used in the fit.
    let (anchor_j, race_from) = if lap_at_triangle {
        if samples.iter().all(|s| s.lap == 0) {
            bail!("the TCX has a single lap: there is no lap press at the triangle");
        }
        let end = samples.iter().rposition(|s| s.lap == 0).unwrap_or(0);
        let j = gps.partition_point(|&i| i <= end).saturating_sub(1); // last GPS point of lap 1
        (Some(j), j + 1)
    } else {
        (None, 0)
    };
    if pts.len() - race_from < ctrl.len() {
        bail!("not enough GPS points after the triangle");
    }
    let sol = solve(&pts[race_from..], ctrl, anchor_j.map(|j| (pts[j], start)))?;
    let sim = sol.sim;
    let scale = sim.scale();
    let visit: Vec<usize> = sol.visit.iter().map(|&i| i + race_from).collect();
    // Leg 1 begins at the lap-1 end if known, else at the track point nearest to the triangle.
    let start_j = anchor_j.unwrap_or_else(|| {
        (0..visit[0].max(1))
            .min_by(|&a, &b| d2(sim.apply(pts[a]), start).total_cmp(&d2(sim.apply(pts[b]), start)))
            .unwrap_or(0)
    });

    // Constant offsets at each anchored point (triangle if known, controls, finish), interpolated
    // in between to cancel GPS drift.
    let mut fidx: Vec<usize> = anchor_j.into_iter().collect();
    let mut fcl: Vec<V> = anchor_j.map(|_| start).into_iter().collect();
    fidx.extend(&visit);
    fcl.extend(ctrl);
    let off: Vec<V> = fidx
        .iter()
        .zip(&fcl)
        .map(|(&i, &c)| {
            let p = sim.apply(pts[i]);
            (c.0 - p.0, c.1 - p.1)
        })
        .collect();
    let offset_at = |i: usize| -> V {
        if i <= fidx[0] {
            return off[0];
        }
        if i >= fidx[fidx.len() - 1] {
            return off[off.len() - 1];
        }
        let c = fidx.partition_point(|&x| x <= i) - 1;
        let f = (i - fidx[c]) as f64 / (fidx[c + 1] - fidx[c]) as f64;
        (off[c].0 + (off[c + 1].0 - off[c].0) * f, off[c].1 + (off[c + 1].1 - off[c].1) * f)
    };

    // Leg of every sample: leg j covers visit[j-1]+1 ..= visit[j]; leg 0 starts at the triangle.
    let visit_sample: Vec<usize> = visit.iter().map(|&j| gps[j]).collect();
    let mut leg_of: Vec<Option<usize>> = vec![None; samples.len()];
    for j in 0..ctrl.len() {
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
            let o = offset_at(j);
            (x, y, xf, yf) = (px(p.0), px(p.1), px(p.0 + o.0), px(p.1 + o.1));
        }
        let leg = leg_of[s].map_or(String::new(), |j| label(j, ctrl.len()));
        let (la, lo) = smp.lat_lon.unzip();
        let pace = (smp.speed > 0.3).then(|| 1000.0 / smp.speed / 60.0);
        w.write_record([
            smp.time_utc.clone(), format!("{:.0}", smp.t), leg, f(la), f(lo), f(smp.ele), f(smp.hr),
            format!("{:.2}", smp.speed), f(pace), format!("{:.1}", smp.dist), x, y, xf, yf,
        ])?;
    }
    w.flush()?;

    let errors_m: Vec<(String, f64)> = visit
        .iter()
        .zip(ctrl)
        .enumerate()
        .map(|(j, (&i, &cl))| (label(j, ctrl.len()), d2(sim.apply(pts[i]), cl).sqrt() / scale))
        .collect();
    let mean_err_m = errors_m.iter().map(|e| e.1).sum::<f64>() / errors_m.len() as f64;
    let max_err_m = errors_m.iter().map(|e| e.1).fold(0.0, f64::max);
    let start_gap_m = d2(sim.apply(pts[start_j]), start).sqrt() / scale;
    Ok(Report { scale_px_per_m: scale, errors_m, mean_err_m, max_err_m, start_gap_m, rows: samples.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local check against a real race. Run with:
    /// VCC_TCX=... VCC_CLICKS="x,y;x,y;..." cargo test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_race() {
        let tcx = std::env::var("VCC_TCX").unwrap();
        let clicks: Vec<V> = std::env::var("VCC_CLICKS")
            .unwrap()
            .split(';')
            .map(|p| {
                let (x, y) = p.split_once(',').unwrap();
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect();
        let samples = crate::tcx::load(Path::new(&tcx)).unwrap();
        let out = std::env::temp_dir().join("vcc-test.csv");
        let lap = std::env::var("VCC_LAP").is_ok();
        let r = calibrate(&samples, &clicks, 1.0, lap, &out).unwrap();
        println!("scale {:.3} px/m, mean {:.1} m, max {:.1} m", r.scale_px_per_m, r.mean_err_m, r.max_err_m);
        for (l, e) in &r.errors_m {
            println!("  {l}: {e:.1} m");
        }
        println!("csv: {}", out.display());
    }
}
