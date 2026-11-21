//! The start menu, for whoever does not use the command line: pick the map and the track, name
//! the map, choose where to save. The race is saved as `<date>-<name>.zip`.

use crate::tcx::Sample;
use crate::{pdf, place};
use anyhow::{Result, bail};
use eframe::egui::{self, Color32};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// What the user chose.
pub enum Choice {
    /// Start a new race from a map and a track.
    New(Request),
    /// Open a race saved as a zip.
    Open(PathBuf),
}

/// A new race, once the user presses Start.
pub struct Request {
    pub map: PathBuf,
    pub tcx: PathBuf,
    pub samples: Vec<Sample>,
    pub date: String,
    pub name: String,
    pub out_dir: PathBuf,
}

impl Request {
    /// Where the race is saved: `<date>-<name>.zip` in the chosen folder.
    pub fn zip(&self) -> Result<PathBuf> {
        let slug = slug(&self.name);
        if slug.is_empty() {
            bail!("le nom de la carte est vide");
        }
        Ok(zip_path(&self.out_dir, &self.date, &slug))
    }
}

/// `<dir>/<date>-<slug>.zip`.
pub fn zip_path(dir: &Path, date: &str, slug: &str) -> PathBuf {
    dir.join(format!("{date}-{slug}.zip"))
}

#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Map,
    Tcx,
    Folder,
    Zip,
}

pub struct Launcher {
    map: String,
    tcx: String,
    out_dir: String,
    name: String,
    /// The track, once the TCX is read, and the day of the race.
    samples: Option<Vec<Sample>>,
    date: Option<String>,
    tcx_error: Option<String>,
    /// The TCX path that was read (or tried), and the map path the name was guessed from.
    tcx_seen: String,
    map_seen: String,
    /// A file dialog is open (it runs in its own thread, so that the window stays alive).
    dialog: Option<(Pick, Receiver<Option<PathBuf>>)>,
    /// A place lookup is running.
    lookup: Option<Receiver<Result<String, String>>>,
    note: String,
    error: Option<String>,
    help_open: bool,
    /// A saved race was chosen with the file dialog.
    open_zip: Option<PathBuf>,
}

impl Launcher {
    pub fn new() -> Self {
        Launcher {
            map: String::new(),
            tcx: String::new(),
            out_dir: String::new(),
            name: String::new(),
            samples: None,
            date: None,
            tcx_error: None,
            tcx_seen: String::new(),
            map_seen: String::new(),
            dialog: None,
            lookup: None,
            note: String::new(),
            error: None,
            help_open: false,
            open_zip: None,
        }
    }

    /// Shows a problem met when starting, and makes the TCX be read again.
    pub fn failed(&mut self, message: String) {
        self.error = Some(message);
        self.tcx_seen.clear();
    }

    fn open_dialog(&mut self, ctx: &egui::Context, what: Pick) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new();
            let picked = match what {
                Pick::Map => dialog
                    .set_title("Carte de la course")
                    .add_filter(
                        "Carte (PDF ou image)",
                        &["pdf", "png", "jpg", "jpeg", "tif", "tiff"],
                    )
                    .pick_file(),
                Pick::Tcx => dialog
                    .set_title("Trace de la course")
                    .add_filter("Trace Garmin (.tcx)", &["tcx"])
                    .pick_file(),
                Pick::Folder => dialog.set_title("Dossier d'enregistrement").pick_folder(),
                Pick::Zip => dialog
                    .set_title("Course enregistrée")
                    .add_filter("Course enregistrée (.zip)", &["zip"])
                    .pick_file(),
            };
            let _ = tx.send(picked);
            ctx.request_repaint();
        });
        self.dialog = Some((what, rx));
    }

    fn poll_dialog(&mut self) {
        let Some((what, rx)) = self.dialog.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(path)) => {
                let text = path.to_string_lossy().into_owned();
                match what {
                    Pick::Map => self.map = text,
                    Pick::Tcx => self.tcx = text,
                    Pick::Folder => self.out_dir = text,
                    Pick::Zip => self.open_zip = Some(path),
                }
            }
            Ok(None) | Err(TryRecvError::Disconnected) => {}
            Err(TryRecvError::Empty) => self.dialog = Some((what, rx)),
        }
    }

    fn poll_lookup(&mut self) {
        let Some(rx) = self.lookup.take() else { return };
        match rx.try_recv() {
            Ok(Ok(name)) => {
                self.name = name;
                self.note.clear();
            }
            Ok(Err(e)) => self.note = e,
            Err(TryRecvError::Empty) => self.lookup = Some(rx),
            Err(TryRecvError::Disconnected) => {}
        }
    }

    /// Reads the TCX when its path changes, and guesses a name from the map file name.
    fn refresh(&mut self) {
        if self.tcx != self.tcx_seen {
            self.tcx_seen = self.tcx.clone();
            self.samples = None;
            self.date = None;
            self.tcx_error = None;
            let path = Path::new(self.tcx.trim());
            if path.is_file() {
                match crate::tcx::load(path) {
                    Ok(samples) => match date_of(&samples) {
                        Some(d) => {
                            self.date = Some(d);
                            self.samples = Some(samples);
                            if self.out_dir.trim().is_empty() {
                                self.out_dir = path
                                    .parent()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                            }
                        }
                        None => self.tcx_error = Some("pas de date dans ce fichier".into()),
                    },
                    Err(e) => self.tcx_error = Some(format!("{e:#}")),
                }
            } else if !self.tcx.trim().is_empty() {
                self.tcx_error = Some("fichier introuvable".into());
            }
        }
        if self.map != self.map_seen {
            self.map_seen = self.map.clone();
            let path = Path::new(self.map.trim());
            if self.name.trim().is_empty() && path.is_file() {
                self.name = guess_name(path);
            }
        }
    }

    /// The saved zip, when everything needed to name it is known.
    fn zip(&self) -> Option<PathBuf> {
        let out = self.out_dir.trim();
        let slug = slug(&self.name);
        if out.is_empty() || slug.is_empty() {
            return None;
        }
        Some(zip_path(Path::new(out), self.date.as_ref()?, &slug))
    }

    /// Draws the menu. Returns the choice once the user presses Start.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Choice> {
        let ctx = ui.ctx().clone();
        self.poll_dialog();
        self.poll_lookup();
        self.refresh();
        let busy = self.dialog.is_some();
        let mut start = false;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Visualiser une course sur sa carte");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("? Aide").clicked() {
                        self.help_open = !self.help_open;
                    }
                });
            });
            ui.add_space(8.0);
            if ui
                .add_enabled(!busy, egui::Button::new("Ouvrir une course enregistrée (.zip)…"))
                .clicked()
            {
                self.open_dialog(&ctx, Pick::Zip);
            }
            ui.add_space(8.0);
            egui::Grid::new("form").num_columns(3).spacing([10.0, 10.0]).show(ui, |ui| {
                ui.label("Carte (PDF ou image numérisée)");
                ui.add(egui::TextEdit::singleline(&mut self.map).desired_width(460.0).hint_text("chemin de la carte"));
                if ui.add_enabled(!busy, egui::Button::new("Parcourir…")).clicked() {
                    self.open_dialog(&ctx, Pick::Map);
                }
                ui.end_row();

                ui.label("Trace (.tcx)");
                ui.add(egui::TextEdit::singleline(&mut self.tcx).desired_width(460.0).hint_text("chemin du fichier .tcx"));
                if ui.add_enabled(!busy, egui::Button::new("Parcourir…")).clicked() {
                    self.open_dialog(&ctx, Pick::Tcx);
                }
                ui.end_row();

                ui.label("Date");
                match (&self.date, &self.tcx_error) {
                    (Some(d), _) => ui.label(format!("{d}  (lue dans la trace)")),
                    (None, Some(e)) => ui.colored_label(Color32::from_rgb(200, 60, 60), e),
                    (None, None) => ui.colored_label(Color32::GRAY, "lue dans la trace"),
                };
                ui.label("");
                ui.end_row();

                ui.label("Nom de la carte");
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(460.0).hint_text("par exemple : rocher dame jouanne"));
                let can_guess = self.samples.is_some() && self.lookup.is_none();
                if ui
                    .add_enabled(can_guess, egui::Button::new("Deviner…"))
                    .on_hover_text("Demande à OpenStreetMap le nom du lieu de la trace. Cela envoie une position sur le réseau : le milieu de la trace, arrondi à environ 100 m.")
                    .clicked()
                {
                    if let Some(center) = self.samples.as_deref().and_then(place::center) {
                        let (tx, rx) = channel();
                        let ctx2 = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(place::lookup(center).map_err(|e| format!("{e:#}")));
                            ctx2.request_repaint();
                        });
                        self.lookup = Some(rx);
                        self.note = "Interrogation d'OpenStreetMap…".into();
                    }
                }
                ui.end_row();


                ui.label("Enregistrer dans");
                ui.add(egui::TextEdit::singleline(&mut self.out_dir).desired_width(460.0).hint_text("dossier, créé s'il n'existe pas"));
                if ui.add_enabled(!busy, egui::Button::new("Parcourir…")).clicked() {
                    self.open_dialog(&ctx, Pick::Folder);
                }
                ui.end_row();
            });
            if !self.note.is_empty() {
                ui.label(&self.note);
            }
            ui.add_space(10.0);

            // What will be saved.
            let zip = self.zip();
            ui.label(egui::RichText::new("Fichier enregistré").strong());
            match &zip {
                Some(path) => {
                    let exists = if path.exists() { "  (existe : sera remplacé)" } else { "" };
                    ui.label(format!("carte, trace et transformation :  {}{exists}", path.display()));
                }
                None => {
                    ui.colored_label(Color32::GRAY, "choisissez la carte et la trace, puis donnez un nom et un dossier");
                }
            }
            ui.add_space(10.0);
            let map_ok = Path::new(self.map.trim()).is_file() && map_extension(Path::new(self.map.trim())).is_some();
            if !self.map.trim().is_empty() && !map_ok {
                ui.colored_label(Color32::from_rgb(200, 60, 60), "la carte doit être un fichier PDF, PNG, JPEG ou TIFF");
            }
            let ready = map_ok && zip.is_some() && self.samples.is_some();
            // Big, centred: grey and disabled until everything is there, then green.
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                let (fill, text) = if ready {
                    (Color32::from_rgb(40, 160, 70), Color32::WHITE)
                } else {
                    (Color32::from_gray(60), Color32::GRAY)
                };
                let label = egui::RichText::new("Démarrer").size(24.0).strong().color(text);
                let button = egui::Button::new(label).fill(fill).min_size(egui::vec2(280.0, 64.0));
                let resp = ui.add_enabled(ready, button);
                if !ready {
                    resp.on_disabled_hover_text("Choisissez la carte et la trace, puis donnez un nom et un dossier");
                } else if resp.clicked() {
                    start = true;
                }
            });
            if let Some(e) = &self.error {
                ui.colored_label(Color32::from_rgb(200, 60, 60), e);
            }
        });
        crate::help::window(&ctx, &mut self.help_open, crate::help::Page::Menu);
        if let Some(path) = self.open_zip.take() {
            self.error = None;
            return Some(Choice::Open(path));
        }
        if !start {
            return None;
        }
        self.error = None;
        Some(Choice::New(Request {
            map: PathBuf::from(self.map.trim()),
            tcx: PathBuf::from(self.tcx.trim()),
            samples: self.samples.take()?,
            date: self.date.clone()?,
            name: self.name.trim().to_string(),
            out_dir: PathBuf::from(self.out_dir.trim()),
        }))
    }
}

/// Extension of a map file, in lower case, if it is a PDF or an image we can read.
pub fn map_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    (ext == "pdf" || pdf::IMAGE_EXTENSIONS.contains(&ext.as_str())).then_some(ext)
}

/// The day of the race, `YYYY-MM-DD`: the day of the first trackpoint (UTC, as written in the file).
pub fn date_of(samples: &[Sample]) -> Option<String> {
    let day = samples.first()?.time_utc.trim().get(..10)?;
    is_day(day).then(|| day.to_string())
}

/// Whether `text` is written `YYYY-MM-DD`.
fn is_day(text: &str) -> bool {
    text.len() == 10
        && text.bytes().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// A name for the map from the name of its file: `2026-11-15-rocher-dame-jouanne.pdf` gives
/// `rocher dame jouanne`.
pub fn guess_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rest = match stem.get(..11) {
        Some(head) if head.ends_with('-') && is_day(&head[..10]) => &stem[11..],
        _ => &stem[..],
    };
    rest.replace(['-', '_'], " ").trim().to_string()
}

/// The name as it is written in file names: lower case, no accents, words joined by `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        let plain: &str = match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => "a",
            'ç' => "c",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ñ' => "n",
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => "o",
            'ù' | 'ú' | 'û' | 'ü' => "u",
            'ý' | 'ÿ' => "y",
            'œ' => "oe",
            'æ' => "ae",
            _ => "",
        };
        if !plain.is_empty() {
            out.push_str(plain);
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_at(day: &str) -> Sample {
        Sample {
            time_utc: day.to_string(),
            lap: 0,
            t: 0.0,
            lat_lon: None,
            ele: None,
            hr: None,
            dist: 0.0,
            speed: 0.0,
        }
    }

    #[test]
    fn slug_is_how_files_are_named() {
        assert_eq!(slug("Rocher Dame Jouanne"), "rocher-dame-jouanne");
        assert_eq!(slug("  Montigny-sur-Loing "), "montigny-sur-loing");
        assert_eq!(slug("Forêt d'Orléans / Ouest"), "foret-d-orleans-ouest");
        assert_eq!(slug("Cœur"), "coeur");
        assert_eq!(slug("???"), "");
    }

    #[test]
    fn date_is_the_day_of_the_first_point() {
        let s = vec![sample_at("2026-11-15T09:03:11.000Z")];
        assert_eq!(date_of(&s).as_deref(), Some("2026-11-15"));
        assert_eq!(date_of(&[sample_at("nonsense here")]), None);
        assert_eq!(date_of(&[]), None);
    }

    #[test]
    fn name_is_guessed_from_the_file_name() {
        assert_eq!(
            guess_name(Path::new("/x/2026-11-15-rocher-dame-jouanne.pdf")),
            "rocher dame jouanne"
        );
        assert_eq!(guess_name(Path::new("/x/scan_foret.png")), "scan foret");
    }

    #[test]
    fn zip_is_named_after_the_date_and_the_name() {
        let req = Request {
            map: PathBuf::new(),
            tcx: PathBuf::new(),
            samples: Vec::new(),
            date: "2026-11-15".into(),
            name: "Rocher Dame Jouanne".into(),
            out_dir: PathBuf::from("/x/y"),
        };
        assert_eq!(req.zip().unwrap(), Path::new("/x/y/2026-11-15-rocher-dame-jouanne.zip"));
        let empty = Request { name: "???".into(), ..req };
        assert!(empty.zip().is_err());
    }
}
