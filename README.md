# visu-course-co

Desktop viewer for orienteering races. It shows your GPS track on the scanned map of the
race and lets you look at it leg by leg: duration, run length, bird length, pace, heart rate
and elevation.

Written in Rust with [egui](https://github.com/emilk/egui).

## Features

- Zoom (mouse wheel) and pan (drag) on the map.
- Select one leg: the view zooms on it, other legs can be faded or hidden.
- Per-leg statistics: duration, run length, bird length (straight line between the
  controls), run / bird ratio, average pace.
- Colour the selected leg by pace or by heart rate.
- Slider along the selected leg, in metres: a dot moves on the map and the time, distance,
  pace, elevation and heart rate at that point are shown, with an elevation profile.
- Calibration mode: click the controls on the map and the Garmin `.tcx` file is placed on
  the map for you; a CSV file is generated.

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

### The map: a PDF

The map of the race, as a scanned PDF. Only the first page is used, and the map should be
north-up.

### The track

Either a CSV file (`--csv`), for example one produced by another tool, or a Garmin `.tcx`
file (`--tcx`) that is calibrated on the map first (see [Calibration](#calibration)).

CSV columns (header names matter, order does not):

| column | required | meaning |
| --- | --- | --- |
| `elapsed_s` | yes | seconds since the start of the recording |
| `to_control` | yes | leg label of the row: `1`, `2`, ..., `F`. Rows with an empty label (before the start, after the finish) are ignored |
| `distance_m` | yes | cumulative distance in metres |
| `x_px_fit`, `y_px_fit` | for the map | position on the map, in pixels of the scan. `x_px`, `y_px` are used if the `_fit` columns are missing. Rows without a position are not drawn |
| `lat`, `lon` | no | needed for the bird length |
| `elevation_m` | no | elevation |
| `heart_rate` | no | heart rate, in bpm |
| `speed_ms` | no | speed in m/s, used for the pace |

A leg is a run of consecutive rows with the same `to_control`.

The pixel coordinates refer to the scan embedded in the PDF, at its native size. If your
file uses another size, pass `--csv-width`.

## Usage

```sh
# view a track given as a CSV (no calibration)
visu-course-co map.pdf --csv track.csv

# calibrate a .tcx on the map, write the CSV, then view it
visu-course-co map.pdf --tcx race.tcx
```

Give exactly one of `--csv` and `--tcx`.

Options:

| option | meaning |
| --- | --- |
| `--csv <file>` | the track as a CSV file, shown as is |
| `--tcx <file>` | a Garmin `.tcx` file, to calibrate on the map first |
| `--controls <N>` | with `--tcx`, the number of controls of the course, the finish not included (default 10) |
| `--out <file>` | with `--tcx`, where to write the CSV (default: `<tcx name>-legs.csv` next to it) |
| `--csv-width <px>` | width in pixels of the page image the CSV coordinates refer to (default: the size of the scan in the PDF) |
| `--render-width <px>` | width used to render the PDF when the poppler fallback is needed (default 4096) |

### Viewer

- **Legs** list on the left: pick a leg, or "All legs". Picking a leg zooms on it.
- **Colour by**: leg colour, pace, or heart rate.
- **Slider** at the bottom (when a leg is selected): move along the leg in metres.
- **Fit to window** shows the whole map again.

## Calibration

Starting from a `.tcx` file (`--tcx`), the app needs to know where the track is on the map.
The side panel lists the **Triangle** (the start), the controls **1, 2, 3, ...** and the **Finish**,
all grey at first. Give the number of controls with `--controls <N>` (default 10), or use
**+** / **−**.

At start the app looks for the course printed on the map (pink or purple circles, the double
circle of the finish, the lines between them and the start triangle) and fills the list with a
first guess: the route is already drawn. Check each point and correct what is wrong, as below.
The note under the list says what was found. The detection needs `--controls` to be right, and
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

When every point is placed, press **Save CSV**. The app finds the scale, rotation
and shift that put the track on your clicks, using the positions at the lap presses. The small
GPS drift between controls is then removed, so that the track goes through every control. The
result is saved as a CSV file, and you stay on the calibration screen. **Open viewer** then shows
the legs (duration, length, pace, heart rate); it needs a saved CSV, and says when changes have
not been saved yet. **Recalibrate**, in the viewer, goes back to the clicks, which are kept, and
so are the laps you ignored.

### The triangle

Two cases, chosen with the tickbox **I pressed lap at the triangle** (under the number of
controls):

- **Ticked:** the lap button was pressed at the triangle, which is then like control 0. It is
  tied to a lap press like every control (the first one, before control 1), so the time at the
  triangle is known and leg 1 starts at that press. The list of laps shows `→ Triangle`.
- **Not ticked:** the triangle only marks where leg 1 begins, and you give the time yourself. Press
  **When did I pass the triangle?**, next to **Save CSV** while calibrating (once the route is
  drawn), or in the side panel of the viewer. A window opens with a slider from the start of the
  watch to the moment you reached control 1. Move it to the time you were at the triangle: the
  track point at that time is put on the triangle, and leg 1 is redrawn from there (the drift is
  spread up to control 1). If the triangle itself is misplaced, press **Place the triangle** and
  click its place on the map. **Done** starts leg 1 at the chosen time and writes the CSV (when
  calibrating, it saves and stays on the calibration screen), **Cancel** keeps the automatic start,
  the track point nearest to the triangle. The button needs a `--tcx` session.

Notes:

- The start triangle is not a control: it marks where leg 1 begins, and the track point at that
  moment sits on it. Whatever the watch recorded before that moment is left out.
- The legs are not independent. The scale, rotation and shift are common to the whole map,
  and the lap presses are tied to the controls together, in order. Only the drift
  correction of a leg depends on its own two controls.
