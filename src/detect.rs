//! Finds the course printed on a scanned map, to propose a first placement of the start, the
//! controls and the finish. The user can then move any point.
//!
//! The overprint is pink or purple. The steps are:
//! 1. a "pinkness" map of the page, at reduced size;
//! 2. Hough voting for rings of the size of a control circle, then a check of each candidate (the
//!    ring is mostly there, and its inside is empty), which drops the hatched areas, text and logos;
//! 3. the finish is the circle that has a second, smaller ring inside;
//! 4. the order of the controls is the path through the circles that follows the most printed line
//!    between consecutive circles, ending at the finish;
//! 5. the start triangle is at the end of the line leaving control 1 that is not the leg to control 2.

use crate::pdf::PageImage;

type V = (f64, f64);

/// Pinkness above which a pixel counts as printed.
const INK: f32 = 0.3;
/// Pinkness above which a pixel counts as part of a line between circles (thinner and fainter).
const LINE_INK: f32 = 0.15;
/// Control circle radius, relative to the page width (about 6 mm on an A4 print).
const RADIUS_REL: f32 = 0.0184;
/// Width at which the page is analysed.
const WORK_WIDTH: usize = 1700;
/// Candidates scoring below this are not circles.
const MIN_SCORE: f32 = 0.45;
/// Candidates with more ink than this in the middle of the ring are not circles.
const MAX_INSIDE: f32 = 0.1;
/// A printed line covering at least this share of the way between two circles links them.
const MIN_LEG_COVER: f32 = 0.65;
/// Most controls whose order is searched (the search is exponential).
const MAX_CONTROLS: usize = 20;

#[derive(Clone, Debug)]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// Share of the ring that is printed, minus a penalty for ink inside.
    pub score: f32,
    /// Share of a second, smaller ring that is printed: high for the finish.
    pub inner: f32,
}

pub struct Detection {
    /// Start, controls 1..n, finish, in page pixels. `None` where nothing was found.
    pub slots: Vec<Option<V>>,
    /// Every circle found, in page pixels (for diagnostics).
    #[cfg_attr(not(test), allow(dead_code))]
    pub circles: Vec<Circle>,
    /// What was found, for the user.
    pub note: String,
}

/// Work image: pinkness in 0..=1.
struct Ink {
    w: usize,
    h: usize,
    /// Reduction factor from the page.
    f: usize,
    p: Vec<f32>,
}

impl Ink {
    fn new(page: &PageImage) -> Ink {
        let f = (page.width / WORK_WIDTH).max(1);
        let (w, h) = (page.width / f, page.height / f);
        let mut p = vec![0.0f32; w * h];
        let n = (f * f) as f32;
        for y in 0..h {
            for x in 0..w {
                let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
                for yy in y * f..y * f + f {
                    let row = (yy * page.width + x * f) * 4;
                    for px in page.rgba[row..row + f * 4].chunks_exact(4) {
                        r += px[0] as f32;
                        g += px[1] as f32;
                        b += px[2] as f32;
                    }
                }
                let (r, g, b) = (r / n, g / n, b / n);
                if r > 140.0 {
                    p[y * w + x] = (((r - g).min(b - g + 25.0)) / 70.0).clamp(0.0, 1.0);
                }
            }
        }
        Ink { w, h, f, p }
    }

    fn at(&self, x: f32, y: f32) -> f32 {
        let (xi, yi) = (x.round() as i64, y.round() as i64);
        if xi < 0 || yi < 0 || xi >= self.w as i64 || yi >= self.h as i64 {
            return 0.0;
        }
        self.p[yi as usize * self.w + xi as usize]
    }

    /// Ink (at least `min`) within 3 pixels of (x, y), across the direction (ux, uy).
    fn near(&self, x: f32, y: f32, (ux, uy): (f32, f32), min: f32) -> bool {
        (-3..=3).any(|k| self.at(x - uy * k as f32, y + ux * k as f32) > min)
    }

    fn blurred(&self) -> Vec<f32> {
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0.0f32; w * h];
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 1..w.saturating_sub(1) {
                tmp[y * w + x] = (self.p[y * w + x - 1] + self.p[y * w + x] + self.p[y * w + x + 1]) / 3.0;
            }
        }
        for y in 1..h.saturating_sub(1) {
            for x in 0..w {
                out[y * w + x] = (tmp[(y - 1) * w + x] + tmp[y * w + x] + tmp[(y + 1) * w + x]) / 3.0;
            }
        }
        out
    }

    /// Share of the ring of radius `r` around (cx, cy) that is printed.
    fn ring_cover(&self, cx: f32, cy: f32, r: f32) -> f32 {
        const N: usize = 72;
        let hit = (0..N)
            .filter(|&k| {
                let t = k as f32 / N as f32 * std::f32::consts::TAU;
                let (c, s) = (t.cos(), t.sin());
                (-2..=2).any(|d| self.at(cx + (r + d as f32) * c, cy + (r + d as f32) * s) > INK)
            })
            .count();
        hit as f32 / N as f32
    }

    /// Mean ink inside the circle of radius `r` around (cx, cy).
    fn disc_ink(&self, cx: f32, cy: f32, r: f32) -> f32 {
        let (mut sum, mut n) = (0.0, 0);
        let ri = r as i32;
        for dy in (-ri..=ri).step_by(2) {
            for dx in (-ri..=ri).step_by(2) {
                if ((dx * dx + dy * dy) as f32) < r * r {
                    sum += self.at(cx + dx as f32, cy + dy as f32);
                    n += 1;
                }
            }
        }
        if n == 0 { 0.0 } else { sum / n as f32 }
    }
}

/// Offsets of points on a circle of radius `r`, about 2 pixels apart.
fn circle_offsets(r: usize) -> Vec<(i32, i32)> {
    let n = ((std::f32::consts::TAU * r as f32) / 2.0).ceil() as usize;
    let mut v: Vec<(i32, i32)> = (0..n)
        .map(|k| {
            let t = k as f32 / n as f32 * std::f32::consts::TAU;
            ((r as f32 * t.cos()).round() as i32, (r as f32 * t.sin()).round() as i32)
        })
        .collect();
    v.dedup();
    v
}

/// Best ring score at each pixel, over the radii, and the radius that gave it.
fn hough(ink: &Ink, radii: &[usize]) -> (Vec<f32>, Vec<f32>) {
    let (w, h) = (ink.w, ink.h);
    let blurred = ink.blurred();
    let mut best = vec![0.0f32; w * h];
    let mut best_r = vec![0.0f32; w * h];
    let mut acc = vec![0.0f32; w * h];
    for &r in radii {
        let offs = circle_offsets(r);
        acc.fill(0.0);
        for y in 0..h {
            for x in 0..w {
                let v = blurred[y * w + x];
                if v < 0.15 {
                    continue;
                }
                for &(dx, dy) in &offs {
                    let (cx, cy) = (x as i32 + dx, y as i32 + dy);
                    if cx >= 0 && cy >= 0 && (cx as usize) < w && (cy as usize) < h {
                        acc[cy as usize * w + cx as usize] += v;
                    }
                }
            }
        }
        let norm = 1.0 / offs.len() as f32;
        for i in 0..w * h {
            let s = acc[i] * norm;
            if s > best[i] {
                best[i] = s;
                best_r[i] = r as f32;
            }
        }
    }
    (best, best_r)
}

/// Rings that really look like control circles, in work-image pixels.
fn find_circles(ink: &Ink) -> Vec<Circle> {
    let r_nom = RADIUS_REL * (ink.w * ink.f) as f32 / ink.f as f32;
    let radii: Vec<usize> = ((r_nom * 0.7) as usize..=(r_nom * 1.35) as usize).step_by(3).collect();
    let (best, best_r) = hough(ink, &radii);

    // Greedy non-maximum suppression of the Hough peaks.
    let mut order: Vec<usize> = (0..best.len()).filter(|&i| best[i] > 0.2).collect();
    order.sort_by(|&a, &b| best[b].total_cmp(&best[a]));
    let sep = r_nom * 0.8;
    let mut peaks: Vec<(f32, f32, f32)> = Vec::new();
    for i in order {
        let (x, y) = ((i % ink.w) as f32, (i / ink.w) as f32);
        if peaks.iter().all(|p| (p.0 - x).hypot(p.1 - y) > sep) {
            peaks.push((x, y, best_r[i]));
            if peaks.len() >= 400 {
                break;
            }
        }
    }

    // Check and refine every peak.
    let mut circles: Vec<Circle> = Vec::new();
    for (px, py, pr) in peaks {
        // Cheap rejection of what is obviously not a ring: ink in the middle (hatched areas).
        if ink.disc_ink(px, py, pr * 0.5) > 2.0 * MAX_INSIDE {
            continue;
        }
        let mut top: Option<Circle> = None;
        for dy in -3..=3 {
            for dx in -3..=3 {
                for dr in -3..=3 {
                    let (x, y, r) = (px + dx as f32, py + dy as f32, pr + dr as f32);
                    let cover = ink.ring_cover(x, y, r);
                    if cover < 0.4 {
                        continue;
                    }
                    // The middle of a control circle is empty. Hatched areas are not.
                    let inside = ink.disc_ink(x, y, r * 0.5);
                    if inside > MAX_INSIDE {
                        continue;
                    }
                    let score = cover - 3.0 * inside;
                    if top.as_ref().is_none_or(|t| score > t.score) {
                        top = Some(Circle { x, y, r, score, inner: 0.0 });
                    }
                }
            }
        }
        let Some(mut c) = top else { continue };
        if c.score < MIN_SCORE {
            continue;
        }
        c.inner = (0.55..=0.8).step_by_f(0.05).map(|k| ink.ring_cover(c.x, c.y, c.r * k)).fold(0.0, f32::max);
        // Two peaks can settle on the same circle.
        match circles.iter_mut().find(|o| (o.x - c.x).hypot(o.y - c.y) < c.r * 0.5) {
            Some(o) => {
                if c.score > o.score {
                    *o = c;
                }
            }
            None => circles.push(c),
        }
    }
    circles.sort_by(|a, b| b.score.total_cmp(&a.score));
    circles
}

trait StepByF {
    fn step_by_f(self, step: f32) -> std::vec::IntoIter<f32>;
}

impl StepByF for std::ops::RangeInclusive<f32> {
    fn step_by_f(self, step: f32) -> std::vec::IntoIter<f32> {
        let mut v = Vec::new();
        let mut x = *self.start();
        while x <= *self.end() + 1e-4 {
            v.push(x);
            x += step;
        }
        v.into_iter()
    }
}

/// Share of the segment from the rim of `a` to the rim of `b` that is printed.
fn leg_cover(ink: &Ink, a: &Circle, b: &Circle) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let d = dx.hypot(dy);
    let (from, to) = (a.r + 2.0, d - b.r - 2.0);
    if to - from < 8.0 {
        return 0.0;
    }
    let u = (dx / d, dy / d);
    let n = (to - from) as usize;
    let hit = (0..n)
        .filter(|&i| {
            let t = from + (to - from) * i as f32 / n as f32;
            ink.near(a.x + u.0 * t, a.y + u.1 * t, u, LINE_INK)
        })
        .count();
    hit as f32 / n as f32
}

/// Score of going from circle `a` to circle `b`: a printed line between them is worth about 2, a
/// hop without a clear line costs more the longer it is. This keeps a faint short leg ahead of a
/// spurious link across the map.
fn hop_score(ink: &Ink, a: &Circle, b: &Circle) -> f32 {
    let cover = leg_cover(ink, a, b);
    if cover >= MIN_LEG_COVER {
        1.0 + cover
    } else {
        -(b.x - a.x).hypot(b.y - a.y) / (10.0 * a.r)
    }
}

/// The order of `ctrl` that follows the most printed lines, ending with the finish `fin`.
/// Returns indices into `ctrl`. The finish being at the end is what gives the direction.
fn order(ink: &Ink, ctrl: &[Circle], fin: &Circle) -> Vec<usize> {
    let n = ctrl.len();
    let w: Vec<Vec<f32>> = (0..n).map(|i| (0..n).map(|j| if i == j { 0.0 } else { hop_score(ink, &ctrl[i], &ctrl[j]) }).collect()).collect();
    let wf: Vec<f32> = ctrl.iter().map(|c| hop_score(ink, c, fin)).collect();
    // dp[mask][last]: best total score of a path through the set `mask`, ending at `last`.
    let size = 1usize << n;
    let mut dp = vec![f32::NEG_INFINITY; size * n];
    let mut from = vec![u8::MAX; size * n];
    for i in 0..n {
        dp[(1 << i) * n + i] = 0.0;
    }
    for mask in 1..size {
        for last in 0..n {
            let cur = dp[mask * n + last];
            if mask >> last & 1 == 0 || cur == f32::NEG_INFINITY {
                continue;
            }
            for next in 0..n {
                if mask >> next & 1 == 1 {
                    continue;
                }
                let (nm, v) = (mask | 1 << next, cur + w[last][next]);
                if v > dp[nm * n + next] {
                    dp[nm * n + next] = v;
                    from[nm * n + next] = last as u8;
                }
            }
        }
    }
    let full = size - 1;
    let last = (0..n).max_by(|&a, &b| (dp[full * n + a] + wf[a]).total_cmp(&(dp[full * n + b] + wf[b]))).unwrap_or(0);
    let mut path = vec![last];
    let (mut mask, mut cur) = (full, last);
    while from[mask * n + cur] != u8::MAX {
        let prev = from[mask * n + cur] as usize;
        mask &= !(1 << cur);
        cur = prev;
        path.push(cur);
    }
    path.reverse();
    path
}

/// Follows a thin printed line leaving circle `c` in direction `t` (radians). The line is allowed
/// to be a few pixels off the centre of the circle and to have small gaps; unlike hatching, it has
/// clear sides. Returns how far it goes and where it ends.
fn follow_line(ink: &Ink, c: &Circle, t: f32) -> (f32, (f32, f32)) {
    let max_len = (ink.w.min(ink.h) as f32) * 0.6;
    let (mut x, mut y, mut h) = (c.x + t.cos() * (c.r + 2.0), c.y + t.sin() * (c.r + 2.0), t);
    let (mut len, mut last, mut gap) = (c.r + 2.0, (x, y), 0);
    while len < max_len && gap <= 6 {
        x += h.cos();
        y += h.sin();
        len += 1.0;
        let (ux, uy) = (h.cos(), h.sin());
        // Where is the ink across the line? Follow it.
        let across: Vec<f32> = (-4..=4).map(|k| k as f32).filter(|k| ink.at(x - uy * k, y + ux * k) > LINE_INK).collect();
        let clear = ink.at(x - uy * 8.0, y + ux * 8.0) < INK || ink.at(x + uy * 8.0, y - ux * 8.0) < INK;
        // Ink on both sides of the line: the triangle (or something else) starts here.
        let both_sides = ink.at(x - uy * 6.0, y + ux * 6.0) > INK && ink.at(x + uy * 6.0, y - ux * 6.0) > INK;
        if both_sides && len > c.r + 10.0 {
            break;
        }
        if across.is_empty() || !clear {
            gap += 1;
            continue;
        }
        let k = across.iter().sum::<f32>() / across.len() as f32;
        x -= uy * 0.5 * k;
        y += ux * 0.5 * k;
        h += 0.03 * k;
        last = (x, y);
        gap = 0;
    }
    (len - gap as f32, last)
}

/// The start triangle: the end of the longest printed line that leaves control 1 in a direction
/// other than the leg to control 2. The line stops at the apex of the triangle, which points at
/// control 1; the centre of the triangle is about one circle radius further.
fn find_start(ink: &Ink, c1: &Circle, c2: &Circle) -> Option<(f32, f32)> {
    let to_c2 = (c2.y - c1.y).atan2(c2.x - c1.x);
    let mut best: Option<(f32, (f32, f32))> = None;
    for k in 0..180 {
        let t = k as f32 / 180.0 * std::f32::consts::TAU;
        let diff = (t - to_c2 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        if diff.abs() < 0.25 {
            continue;
        }
        let (len, end) = follow_line(ink, c1, t);
        if best.is_none_or(|b| len > b.0) {
            best = Some((len, end));
        }
    }
    let (len, end) = best?;
    if len < 4.0 * c1.r {
        return None;
    }
    let (dx, dy) = (end.0 - c1.x, end.1 - c1.y);
    let d = dx.hypot(dy);
    Some((end.0 + dx / d * c1.r, end.1 + dy / d * c1.r))
}

/// Proposes a placement of the start, `n_controls` controls and the finish on the map.
pub fn detect(page: &PageImage, n_controls: usize) -> Detection {
    let mut slots: Vec<Option<V>> = vec![None; n_controls + 2];
    let ink = Ink::new(page);
    let circles = find_circles(&ink);
    let to_page = |x: f32, y: f32| ((x as f64 + 0.5) * ink.f as f64, (y as f64 + 0.5) * ink.f as f64);
    let scaled: Vec<Circle> = circles
        .iter()
        .map(|c| Circle { x: (c.x + 0.5) * ink.f as f32, y: (c.y + 0.5) * ink.f as f32, r: c.r * ink.f as f32, ..c.clone() })
        .collect();
    let mut det = Detection { slots: slots.clone(), circles: scaled, note: String::new() };

    // The finish has a second ring inside.
    let Some(fin_i) = (0..circles.len()).max_by(|&a, &b| circles[a].inner.total_cmp(&circles[b].inner)).filter(|&i| circles[i].inner > 0.5)
    else {
        det.note = format!("Auto-placement: {} circles found, but not the finish (double circle).", circles.len());
        return det;
    };
    let fin = circles[fin_i].clone();
    let ctrl: Vec<Circle> = circles.iter().enumerate().filter(|&(i, _)| i != fin_i).map(|(_, c)| c.clone()).take(n_controls).collect();
    if ctrl.len() < n_controls {
        det.note = format!("Auto-placement: found {} of {} controls, nothing placed.", ctrl.len(), n_controls);
        return det;
    }
    if n_controls > MAX_CONTROLS {
        det.note = format!("Auto-placement: too many controls (more than {MAX_CONTROLS}).");
        return det;
    }
    let path = order(&ink, &ctrl, &fin);
    for (k, &i) in path.iter().enumerate() {
        slots[k + 1] = Some(to_page(ctrl[i].x, ctrl[i].y));
    }
    slots[n_controls + 1] = Some(to_page(fin.x, fin.y));
    let start = find_start(&ink, &ctrl[path[0]], &ctrl[*path.get(1).unwrap_or(&path[0])]);
    slots[0] = start.map(|(x, y)| to_page(x, y));
    det.note = format!(
        "Auto-placement: {} controls and the finish found{}; check the order and move what is wrong.",
        n_controls,
        if start.is_some() { ", and the start" } else { " (start not found)" },
    );
    det.slots = slots;
    det
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local check on a real map. Run with:
    /// VCC_PDF=map.pdf VCC_N=15 VCC_OVERLAY=out.png cargo test --release detect -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_map() {
        let pdf = std::env::var("VCC_PDF").unwrap();
        let n: usize = std::env::var("VCC_N").unwrap().parse().unwrap();
        let map = crate::pdf::load_map(std::path::Path::new(&pdf), 4096).unwrap();
        let t = std::time::Instant::now();
        let det = detect(&map.image, n);
        println!("detection took {:?}", t.elapsed());
        println!("{}", det.note);
        for c in &det.circles {
            println!("circle x={:.0} y={:.0} r={:.0} score={:.2} inner={:.2}", c.x, c.y, c.r, c.score, c.inner);
        }
        for (k, s) in det.slots.iter().enumerate() {
            println!("slot {k}: {s:?}");
        }
        if let Ok(tcx) = std::env::var("VCC_TCX") {
            // Does the detected placement give a good fit of the track?
            let samples = crate::tcx::load(std::path::Path::new(&tcx)).unwrap();
            let r = crate::calib::calibrate(&samples, &det.slots, 1.0, None, &[], None).unwrap();
            println!("fit: mean {:.1} m, max {:.1} m", r.mean_err_m, r.max_err_m);
            for (l, e) in &r.errors_m {
                println!("  {l}: {e:.1} m");
            }
            println!("lap based: {}", r.lap_based);
            for l in &r.laps {
                println!("  lap {:2} t={:5.0}s stop={} slot={:?}", l.id + 1, l.t, l.is_stop, l.slot);
            }
        }
        if let Ok(out) = std::env::var("VCC_OVERLAY") {
            let img = &map.image;
            let mut im = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.rgba.clone()).unwrap();
            let mut dot = |x: f64, y: f64, rad: i32, col: [u8; 4]| {
                for dy in -rad..=rad {
                    for dx in -rad..=rad {
                        let (px, py) = (x as i32 + dx, y as i32 + dy);
                        if dx * dx + dy * dy <= rad * rad && px >= 0 && py >= 0 && (px as u32) < im.width() && (py as u32) < im.height() {
                            im.put_pixel(px as u32, py as u32, image::Rgba(col));
                        }
                    }
                }
            };
            for c in &det.circles {
                for k in 0..360 {
                    let t = (k as f32).to_radians();
                    dot((c.x + c.r * t.cos()) as f64, (c.y + c.r * t.sin()) as f64, 2, [0, 120, 255, 255]);
                }
            }
            let placed: Vec<V> = det.slots.iter().flatten().copied().collect();
            for w in placed.windows(2) {
                for k in 0..200 {
                    let f = k as f64 / 200.0;
                    dot(w[0].0 + (w[1].0 - w[0].0) * f, w[0].1 + (w[1].1 - w[0].1) * f, 2, [0, 0, 0, 255]);
                }
            }
            for (k, p) in placed.iter().enumerate() {
                let col = if k == 0 { [0, 200, 0, 255] } else if k + 1 == placed.len() { [255, 0, 0, 255] } else { [255, 220, 0, 255] };
                dot(p.0, p.1, 6 + 3 * k as i32 / 2, col);
            }
            im.save(out).unwrap();
        }
    }
}
