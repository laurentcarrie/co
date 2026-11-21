//! A saved race: one zip file, named after the race (for instance `2026-11-15-rocher-dame-jouanne.zip`),
//! with the map, the track of the watch and everything the user decided:
//!
//! - `map.pdf` (or `map.png`, `map.jpg`, `map.tif`): the map,
//! - `race.tcx`: the track of the watch,
//! - `transformation.yml`: how the track sits on the map, and the choices that led to it.
//!
//! The legs are not stored: they are computed from these three files when the race is opened.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const FORMAT_VERSION: u32 = 1;
const TRANSFORMATION: &str = "transformation.yml";
const TCX: &str = "race.tcx";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Transformation {
    pub version: u32,
    pub race: Race,
    pub sources: Sources,
    /// GPS to metres: a flat projection around (`lat0`, `lon0`):
    /// `X = (lon - lon0) * cos(lat0) * R` (east), `Y = -(lat - lat0) * R` (south).
    pub projection: Projection,
    /// Metres to map pixels, as complex numbers: `w = a * z + b`, that is
    /// `x = a0*X - a1*Y + b0` and `y = a0*Y + a1*X + b1`. Derived from `points` and `watch`: written to be
    /// used by other tools, recomputed when the race is opened.
    pub similarity: Similarity,
    /// Where the user placed the triangle, the controls and the finish, in pixels of the map image.
    pub points: Vec<Point>,
    /// What the user decided about the lap presses of the watch.
    pub watch: Watch,
    /// When the watch was at the start and at each control and the finish. Derived, like `similarity`.
    pub visits: Visits,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Race {
    pub name: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Number of controls, the start and the finish not included.
    pub controls: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Sources {
    pub map: MapSource,
    pub tcx: TcxSource,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MapSource {
    pub file: String,
    pub sha256: String,
    /// Width in pixels of the map image the points refer to.
    pub width_px: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TcxSource {
    pub file: String,
    pub sha256: String,
    pub samples: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Projection {
    pub lat0: f64,
    pub lon0: f64,
    pub earth_radius_m: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Similarity {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub scale_px_per_m: f64,
    pub rotation_deg: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Point {
    /// `triangle`, `1`, `2`, ..., `finish`.
    pub name: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Watch {
    /// The lap button was pressed at the triangle.
    pub lap_at_triangle: bool,
    /// When the triangle was passed, in seconds since the first point of the track, if the user said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time_s: Option<f64>,
    /// Lap presses made by mistake (lap numbers, from 1).
    #[serde(default)]
    pub ignored_laps: Vec<usize>,
    /// Lap presses moved by the user: lap number (from 1) -> seconds since the first point of the track.
    #[serde(default)]
    pub lap_moves: BTreeMap<usize, f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Visits {
    /// Seconds since the first point of the track, when leg 1 starts.
    pub start_s: f64,
    /// Seconds, for each control and then the finish.
    pub controls_s: Vec<f64>,
}

fn sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Sources {
    pub fn new(map_ext: &str, map: &[u8], width_px: u32, tcx: &[u8], samples: usize) -> Self {
        Sources {
            map: MapSource {
                file: format!("map.{map_ext}"),
                sha256: sha256(map),
                width_px,
            },
            tcx: TcxSource {
                file: TCX.to_string(),
                sha256: sha256(tcx),
                samples,
            },
        }
    }
}

/// A race read from its zip file.
pub struct Loaded {
    pub transformation: Transformation,
    pub map: Vec<u8>,
    pub map_ext: String,
    pub tcx: Vec<u8>,
    /// Things that look wrong (the files do not match what the transformation was made for).
    pub warnings: Vec<String>,
}

/// Writes the zip. The file is written aside and then moved, so that a failure keeps the previous one.
pub fn save(path: &Path, map: &[u8], tcx: &[u8], t: &Transformation) -> Result<()> {
    let yaml = serde_yaml::to_string(t).context("impossible d'écrire transformation.yml")?;
    let tmp = path.with_extension("zip.tmp");
    let write = || -> Result<()> {
        let file = std::fs::File::create(&tmp)
            .with_context(|| format!("impossible d'écrire {}", tmp.display()))?;
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file(t.sources.map.file.as_str(), opts)?;
        zip.write_all(map)?;
        zip.start_file(TCX, opts)?;
        zip.write_all(tcx)?;
        zip.start_file(TRANSFORMATION, opts)?;
        zip.write_all(yaml.as_bytes())?;
        zip.finish()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("impossible d'écrire {}", path.display()))?;
    Ok(())
}

/// Reads the zip of a race.
pub fn load(path: &Path) -> Result<Loaded> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("impossible d'ouvrir {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("{} n'est pas un fichier zip", path.display()))?;
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().and_then(|f| f.name().ok().map(|n| n.into_owned())))
        .collect();
    let map_name = names
        .iter()
        .find(|n| n.starts_with("map."))
        .cloned()
        .context("la carte (map.pdf) manque dans le zip")?;
    let mut read = |name: &str| -> Result<Vec<u8>> {
        let mut entry = zip
            .by_name(name)
            .with_context(|| format!("{name} manque dans le zip"))?;
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        Ok(data)
    };
    let map = read(&map_name)?;
    let tcx = read(TCX)?;
    let yaml = read(TRANSFORMATION)?;
    let transformation: Transformation =
        serde_yaml::from_slice(&yaml).context("transformation.yml est invalide")?;
    if transformation.version > FORMAT_VERSION {
        bail!(
            "transformation.yml est d'une version plus récente ({}) que ce programme ({FORMAT_VERSION})",
            transformation.version
        );
    }
    let mut warnings = Vec::new();
    if sha256(&map) != transformation.sources.map.sha256 {
        warnings.push("la carte n'est pas celle de la transformation".to_string());
    }
    if sha256(&tcx) != transformation.sources.tcx.sha256 {
        warnings.push("la trace n'est pas celle de la transformation".to_string());
    }
    let map_ext = map_name.rsplit('.').next().unwrap_or("pdf").to_lowercase();
    Ok(Loaded {
        transformation,
        map,
        map_ext,
        tcx,
        warnings,
    })
}

/// A scratch folder with the map written in it, for the code that reads files. Removed when dropped.
pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("visu-course-co-{}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("impossible de créer {}", dir.display()))?;
        Ok(Scratch { dir })
    }

    /// Writes `data` as `name` in the folder and returns its path.
    pub fn write(&self, name: &str, data: &[u8]) -> Result<PathBuf> {
        let path = self.dir.join(name);
        std::fs::write(&path, data)
            .with_context(|| format!("impossible d'écrire {}", path.display()))?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Transformation {
        Transformation {
            version: FORMAT_VERSION,
            race: Race {
                name: "Rocher Dame Jouanne".into(),
                date: "2026-11-15".into(),
                controls: 2,
            },
            sources: Sources::new("pdf", b"map", 2544, b"tcx", 100),
            projection: Projection {
                lat0: 48.4,
                lon0: 2.6,
                earth_radius_m: 6_371_000.0,
            },
            similarity: Similarity {
                a: [0.9, 0.01],
                b: [100.0, 200.0],
                scale_px_per_m: 0.9,
                rotation_deg: 0.6,
            },
            points: vec![
                Point { name: "triangle".into(), x: 1.0, y: 2.0 },
                Point { name: "finish".into(), x: 3.0, y: 4.0 },
            ],
            watch: Watch {
                lap_at_triangle: false,
                start_time_s: Some(412.0),
                ignored_laps: vec![3],
                lap_moves: BTreeMap::from([(4, 1873.0)]),
            },
            visits: Visits { start_s: 412.0, controls_s: vec![500.0, 900.0] },
        }
    }

    #[test]
    fn zip_round_trip() {
        let path = std::env::temp_dir().join("vcc-bundle-test.zip");
        let t = sample();
        save(&path, b"map", b"tcx", &t).unwrap();
        let l = load(&path).unwrap();
        assert_eq!(l.map, b"map");
        assert_eq!(l.tcx, b"tcx");
        assert_eq!(l.map_ext, "pdf");
        assert!(l.warnings.is_empty());
        assert_eq!(l.transformation.watch.lap_moves[&4], 1873.0);
        assert_eq!(l.transformation.points[1].name, "finish");
        assert_eq!(l.transformation.visits.controls_s, [500.0, 900.0]);
        let _ = std::fs::remove_file(&path);
    }
}
