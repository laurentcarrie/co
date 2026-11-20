//! Minimal Garmin TCX reader: one `Sample` per trackpoint.

use anyhow::{Context, Result};
use roxmltree::Node;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Sample {
    /// Timestamp exactly as written in the file.
    pub time_utc: String,
    /// Seconds since the first sample.
    pub t: f64,
    pub lat_lon: Option<(f64, f64)>,
    pub ele: Option<f64>,
    pub hr: Option<f64>,
    /// Cumulative distance in metres.
    pub dist: f64,
    /// Raw speed in m/s.
    pub speed: f64,
}

fn local<'a>(n: &Node<'a, 'a>) -> &'a str {
    n.tag_name().name()
}

fn find<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.descendants().find(|d| d.is_element() && local(d) == name)
}

fn num(n: Node, name: &str) -> Option<f64> {
    find(n, name)?.text()?.trim().parse().ok()
}

/// Seconds since the Unix epoch for `YYYY-MM-DDTHH:MM:SS[.fff]Z`.
fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let h: f64 = t.next()?.parse().ok()?;
    let mi: f64 = t.next()?.parse().ok()?;
    let sec: f64 = t.next()?.parse().ok()?;
    // days_from_civil
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days as f64 * 86400.0 + h * 3600.0 + mi * 60.0 + sec)
}

pub fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6_371_000.0;
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = la2 - la1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

pub fn load(path: &Path) -> Result<Vec<Sample>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let doc = roxmltree::Document::parse(&text).context("invalid TCX (XML) file")?;
    let mut out: Vec<Sample> = Vec::new();
    let mut t0 = None;
    for tp in doc.descendants().filter(|n| n.is_element() && local(n) == "Trackpoint") {
        let Some(time) = find(tp, "Time").and_then(|n| n.text()).map(|s| s.trim().to_string()) else { continue };
        let Some(abs) = parse_time(&time) else { continue };
        let t = abs - *t0.get_or_insert(abs);
        let lat_lon = match (num(tp, "LatitudeDegrees"), num(tp, "LongitudeDegrees")) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        let prev = out.last();
        let dist = num(tp, "DistanceMeters").unwrap_or_else(|| {
            let step = match (prev.and_then(|p| p.lat_lon), lat_lon) {
                (Some(a), Some(b)) => haversine_m(a, b),
                _ => 0.0,
            };
            prev.map_or(0.0, |p| p.dist) + step
        });
        let speed = num(tp, "Speed").unwrap_or_else(|| match prev {
            Some(p) if t > p.t => (dist - p.dist) / (t - p.t),
            _ => 0.0,
        });
        let hr = find(tp, "HeartRateBpm").and_then(|h| num(h, "Value"));
        out.push(Sample { time_utc: time, t, lat_lon, ele: num(tp, "AltitudeMeters"), hr, dist, speed });
    }
    anyhow::ensure!(!out.is_empty(), "no trackpoints in {}", path.display());
    Ok(out)
}
