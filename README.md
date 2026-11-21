# visu-course-co

Desktop viewer for orienteering races. It shows your GPS track on the scanned map of the
race and lets you look at it leg by leg: duration, run length, bird length, pace, heart rate
and elevation.

Written in Rust with [egui](https://github.com/emilk/egui).

## Features

- The interface is in French. The **? Aide** button, on every screen, opens a help window (in
  French too) that explains the screen you are on. The command-line help and this README are in
  English.
- Zoom (mouse wheel) and pan (drag) on the map.
- Select one leg: the view zooms on it, other legs can be faded or hidden.
- **Exporter en PDF** saves the map with the legs drawn on it, next to the zip, as `<date>-<name>-export.pdf`.
- Per-leg statistics: duration, run length, bird length (straight line between the
  controls), run / bird ratio, average pace.
- Colour the selected leg by pace or by heart rate, with min and max sliders for the colour scale.
- Slider for the width of the track line.
- Slider along the selected leg, in metres: a dot moves on the map and the time, distance,
  pace, elevation and heart rate at that point are shown, with an elevation profile.
- Calibration mode: click the controls on the map and the Garmin `.tcx` file is placed on
  the map for you; the race is saved as one zip file.

## Build

You need a [Rust toolchain](https://rustup.rs) (edition 2024, Rust 1.85 or newer).

```sh
cargo build --release
```

The binary is `target/release/visu-course-co`. It runs on a desktop with OpenGL support.

The map is read from the PDF in pure Rust. For PDFs where that is not possible (for example
a vector map), `pdftoppm` and `pdfimages` from poppler (`poppler-utils`) are used as a
fallback, if installed.

## Input files

### The map: a PDF or an image

The map of the race, as a scanned PDF (only the first page is used), or as a scan saved as a PNG,
JPEG or TIFF image. The map should be north-up.

### The track

A Garmin `.tcx` file exported by the watch. It is placed on the map by clicking the controls (see
[Calibration](#calibration)).

### The saved race

A race is saved as one zip file, `<date>-<name>.zip` (for example
`2026-11-15-rocher-dame-jouanne.zip`), with:

- `map.pdf` (or `map.png`, `map.jpg`, `map.tif`): the map;
- `race.tcx`: the track of the watch;
- `transformation.yml`: how the track sits on the map, and the choices that led to it.

The legs are not stored: they are computed from these files when the race is opened.

`transformation.yml` holds:

| key | meaning |
| --- | --- |
| `race` | name, date, number of controls |
| `sources` | file names, SHA-256 and sizes of the map and the track, to check that they match |
| `projection` | the flat projection from GPS to metres: `X = (lon - lon0) * cos(lat0) * R` (east), `Y = -(lat - lat0) * R` (south) |
| `similarity` | metres to map pixels, `w = a * z + b` as complex numbers (scale, rotation, shift). Derived: written for other tools, recomputed on opening |
| `points` | where you placed the triangle, the controls and the finish, in pixels of the map image |
| `watch` | your decisions: lap pressed at the triangle, when you passed the triangle (seconds), ignored laps, laps you moved (seconds) |
| `visits` | when the watch was at the start and at each control and the finish, in seconds since the first point of the track. Derived |

Times are seconds since the first point of the track. Laps are numbered from 1, as in the app.
To get millimetres on paper from the map pixels, divide by the resolution of the scan:
`mm = px * 25.4 / dpi`.

## Usage

### Start menu

Run `visu-course-co` without arguments (or start it from the desktop): a menu asks for

- the **map**: a PDF or a scanned image (PNG, JPEG, TIFF), with a **Search…** button;
- the **track**: the Garmin `.tcx` file, with a **Search…** button;
- the **date**, read from the track (`YYYY-MM-DD`, the day of the first point, as written in the
  file, in UTC);
- the **name of the map**, which you type. It is guessed from the name of the map file when that
  is `<date>-<name>`. **Guess…** asks OpenStreetMap for the name of the place of the track: it
  sends one position over the network (the middle of the track, rounded to about 100 m), and
  only when you press it;
- the folder to **save in**, created if missing.

The race is saved in that folder as `<date>-<name>.zip`, with the name written in lower case,
without accents, words joined by `-`. The menu shows the file name before you press **Start**, and
says when it already exists and will be replaced. Nothing is written until you press **Save**.
Then the calibration starts, as described below, and a window asks how many controls the course
has (the circles seen on the map give the number to suggest).

**Open a saved race (.zip)…** loads a race that was saved: the map, the track and your choices are
read back and you arrive in view mode.

### Command line

```sh
# open a saved race
visu-course-co 2026-11-15-rocher-dame-jouanne.zip

# start a new race: calibrate a .tcx on the map
visu-course-co map.pdf --tcx race.tcx --name "rocher dame jouanne"
```

The map can be a PDF or a PNG, JPEG or TIFF image.

Options (for a new race):

| option | meaning |
| --- | --- |
| `--tcx <file>` | the Garmin `.tcx` file, to calibrate on the map |
| `--name <name>` | the name of the race (default: guessed from the map file name) |
| `--out <file>` | where to save the zip (default: `<date>-<name>.zip` next to the `.tcx`) |
| `--controls <N>` | the number of controls of the course, the finish not included (if not given, a window asks for it at start) |
| `--lookup-place` | when no `--name` is given, ask OpenStreetMap for the name of the place of the track (a village or town near it). This is the only use of the network: it sends one position, the middle of the track rounded to about 100 m. It is not the name printed on the map |
| `--render-width <px>` | width used to render the PDF when the poppler fallback is needed (default 4096) |

### Viewer

- **Legs** list on the left: pick a leg, or "All legs". Picking a leg zooms on it.
- **Colour by**: leg colour, pace, or heart rate.
- **Slider** at the bottom (when a leg is selected): move along the leg in metres.
- **Fit to window** shows the whole map again.

## Calibration

Starting from a `.tcx` file, the app needs to know where the track is on the map.
The side panel lists the **Triangle** (the start), the controls **1, 2, 3, ...** and the **Finish**,
all grey at first. A window asks how many controls the course has, with the number of circles seen on the map as the
suggestion (or give `--controls <N>`); change it later with
**+** / **−**.

At start the app looks for the course printed on the map (pink or purple circles, the double
circle of the finish, the lines between them and the start triangle) and fills the list with a
first guess: the route is already drawn. Check each point and correct what is wrong, as below.
The note under the list says what was found. The detection needs the number of controls to be right, and
a legible overprint; if it cannot find everything, nothing is placed and you click the points
yourself. The order comes from the lines between the circles, so check it where two circles
are close to each other.

1. Click a row of the list: it is the point you are about to place.
2. Click its position on the map. The next point still to place is then selected.
3. Click a row again to move that point, or **✕** to remove it.

As soon as the triangle and the finish are placed, the route is drawn on the map, and it is
updated every time a control is placed. With only the triangle and the finish it is a rough
guess (the scale is not known); a few controls make it right. The side panel shows the mean
and the worst error in metres: a large error usually means a misplaced click.

### Watch laps

A control is visited when you pressed the lap button, so every leg ends at a lap press of the
watch, not at the track point nearest to the control (you may pass close to a control without
being there). The app ties the controls, in order, to the lap presses that fit them best. The
end of the recording (the stop of the watch) counts as a last press, for the finish.

The lap presses are listed in the side panel ("Watch laps") and drawn on the route: a white dot
with the number of its control, a grey `?` for a press that is not used. If you pressed lap by
mistake, a press that belongs to no control is simply left out; if the app ties a control to the
wrong press, untick that press in the list, or right-click it on the map, and the controls are
tied again without it. If the file has fewer lap presses than controls, the nearest track points
are used instead, and the panel says so.

### Saving

When every point is placed, press **Save**. The app finds the scale, rotation
and shift that put the track on your clicks, using the positions at the lap presses. The small
GPS drift between controls is then removed, so that the track goes through every control. The
race is saved as the zip file (map, track and transformation), and you stay on the calibration
screen. **VISUALISATION** then shows the legs (duration, length, pace, heart rate), and says when
changes have not been saved yet. **ÉDITION** goes back to the clicks, which are kept, and so are
the laps you ignored or moved.

### The triangle

Two cases, chosen with the tickbox **I pressed lap at the triangle** (under the number of
controls):

- **Ticked:** the lap button was pressed at the triangle, which is then like control 0. It is
  tied to a lap press like every control (the first one, before control 1), so the time at the
  triangle is known and leg 1 starts at that press. The list of laps shows `→ Triangle`.
- **Not ticked:** the triangle only marks where leg 1 begins, and you give the time yourself. Press
  **When did I pass the triangle?**, next to **Save** while calibrating (once the route is
  drawn), or in the side panel of the viewer. A window opens with a slider from the start of the
  watch to the moment you reached control 1. Move it to the time you were at the triangle: the
  track point at that time is put on the triangle, and leg 1 is redrawn from there (the drift is
  spread up to control 1). If the triangle itself is misplaced, press **Place the triangle** and
  click its place on the map. **Done** starts leg 1 at the chosen time and saves (when
  calibrating, it stays on the calibration screen), **Cancel** keeps the automatic start,
  the track point nearest to the triangle. 

Notes:

- The start triangle is not a control: it marks where leg 1 begins, and the track point at that
  moment sits on it. Whatever the watch recorded before that moment is left out.
- The legs are not independent. The scale, rotation and shift are common to the whole map,
  and the lap presses are tied to the controls together, in order. Only the drift
  correction of a leg depends on its own two controls.
