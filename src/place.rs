//! Guesses the name of the place of a track, with OpenStreetMap (Nominatim reverse geocoding).
//!
//! This is the only thing in the app that uses the network, and it is opt-in (`--lookup-place`).
//! What leaves the machine is one position, the middle of the track rounded to about 100 m, and a
//! generic User-Agent. The answer is the name of a village or a town near the track, not the name
//! printed on the map by the club.

use crate::tcx::Sample;
use anyhow::{Context, Result};
use std::time::Duration;

/// The middle of the track (median latitude and longitude), rounded to 3 decimals (about 100 m).
pub fn center(samples: &[Sample]) -> Option<(f64, f64)> {
    let median = |mut v: Vec<f64>| -> Option<f64> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    };
    let lat = median(
        samples
            .iter()
            .filter_map(|s| s.lat_lon.map(|p| p.0))
            .collect(),
    )?;
    let lon = median(
        samples
            .iter()
            .filter_map(|s| s.lat_lon.map(|p| p.1))
            .collect(),
    )?;
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    Some((round(lat), round(lon)))
}

/// Asks OpenStreetMap for the name of the place at `center`.
pub fn lookup(center: (f64, f64)) -> Result<String> {
    let url = format!(
        "https://nominatim.openstreetmap.org/reverse?format=jsonv2&addressdetails=1&zoom=14&lat={:.3}&lon={:.3}",
        center.0, center.1
    );
    // Trust the certificates of the system, which include those of a company network.
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .tls_config(tls)
        .build()
        .into();
    let body = agent
        .get(&url)
        .header("User-Agent", "visu-course-co")
        .call()
        .context("impossible de joindre OpenStreetMap")?
        .body_mut()
        .read_to_string()
        .context("impossible de lire la réponse d'OpenStreetMap")?;
    if std::env::var("VCC_DEBUG").is_ok() {
        eprintln!("{body}");
    }
    parse(&body)
}

/// The name of the place in a Nominatim answer: a village, a town or the like.
fn parse(body: &str) -> Result<String> {
    let json: serde_json::Value =
        serde_json::from_str(body).context("réponse inattendue d'OpenStreetMap")?;
    if let Some(e) = json.get("error").and_then(|e| e.as_str()) {
        anyhow::bail!("OpenStreetMap : {e}");
    }
    let address = json.get("address");
    let field = |k: &str| address.and_then(|a| a.get(k)).and_then(|v| v.as_str());
    [
        "village",
        "town",
        "city",
        "municipality",
        "hamlet",
        "suburb",
        "locality",
        "county",
    ]
    .iter()
    .find_map(|k| field(k))
    .or_else(|| {
        json.get("name")
            .and_then(|n| n.as_str())
            .filter(|n| !n.is_empty())
    })
    .map(str::to_string)
    .context("pas de nom de lieu dans la réponse d'OpenStreetMap")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(lat: f64, lon: f64) -> Sample {
        Sample {
            time_utc: String::new(),
            lap: 0,
            t: 0.0,
            lat_lon: Some((lat, lon)),
            ele: None,
            hr: None,
            dist: 0.0,
            speed: 0.0,
        }
    }

    #[test]
    fn center_is_rounded_to_about_100_m() {
        let s = vec![
            sample(48.40012, 2.70198),
            sample(48.40461, 2.70331),
            sample(48.99, 2.0),
        ];
        assert_eq!(center(&s), Some((48.405, 2.702)));
        assert_eq!(center(&[]), None);
    }

    #[test]
    fn parses_the_place_of_an_answer() {
        let body = r#"{"name":"Rue X","address":{"road":"Rue X","village":"Montigny-sur-Loing","county":"Seine-et-Marne"}}"#;
        assert_eq!(parse(body).unwrap(), "Montigny-sur-Loing");
        let body = r#"{"name":"Forêt","address":{"county":"Seine-et-Marne"}}"#;
        assert_eq!(parse(body).unwrap(), "Seine-et-Marne");
        assert!(parse(r#"{"error":"Unable to geocode"}"#).is_err());
    }

    #[test]
    fn prefers_the_village_to_the_hamlet() {
        // Answer for the middle of a track in the forest of Fontainebleau.
        let body = r#"{"name":"Busseau","addresstype":"hamlet","address":{"hamlet":"Busseau","village":"Villiers-sous-Grez","municipality":"Fontainebleau","county":"Seine-et-Marne"}}"#;
        assert_eq!(parse(body).unwrap(), "Villiers-sous-Grez");
    }

    /// One real request. Run with: VCC_TCX=race.tcx cargo test place -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_lookup() {
        let samples =
            crate::tcx::load(std::path::Path::new(&std::env::var("VCC_TCX").unwrap())).unwrap();
        let c = center(&samples).unwrap();
        println!("sending {c:?}");
        println!("place: {}", lookup(c).unwrap());
    }
}
