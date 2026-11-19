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

Either a CSV file, or a Garmin `.tcx` file that is calibrated on the map first (see
[Calibration](#calibration)).

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
# view a track that already has a CSV
visu-course-co map.pdf track.csv

# calibrate a .tcx on the map, write the CSV, then view it
visu-course-co map.pdf race.tcx
```

Options:

| option | meaning |
| --- | --- |
| `--out <file>` | where to write the CSV when the input is a `.tcx` (default: `<tcx name>-legs.csv` next to it) |
| `--csv-width <px>` | width in pixels of the page image the CSV coordinates refer to (default: the size of the scan in the PDF) |
| `--render-width <px>` | width used to render the PDF when the poppler fallback is needed (default 4096) |

### Viewer

- **Legs** list on the left: pick a leg, or "All legs". Picking a leg zooms on it.
- **Colour by**: leg colour, pace, or heart rate.
- **Slider** at the bottom (when a leg is selected): move along the leg in metres.
- **Fit to window** shows the whole map again.

## Calibration

Starting from a `.tcx` file, the app needs to know where the track is on the map. Click on
the map, in this order:

1. the start triangle,
2. control 1, 2, 3, ...,
3. the finish.

Then press **Compute**. The app finds the scale, rotation and shift that put the track on
your clicks, and works out when you reached each control, so you do not have to mark the
time of each control. The small GPS drift between controls is then removed, so that the
track goes through every control. The result is saved as a CSV file and opened in the
viewer. After computing, the side panel shows the mean and the worst error in metres:
a large error usually means a misplaced click. **Recalibrate** goes back to the clicks.

Notes:

- The start triangle is not a control. By default it only marks where leg 1 begins (the
  nearest point of the track before control 1) and is not used for the fit. Whatever the
  watch recorded before that point is left out.
- Tick **I pressed lap at the triangle** if you pressed the lap button of the watch when
  you were at the triangle. The first watch lap then ends at the triangle: that gives the
  exact time of the start, the triangle is used in the fit, and everything before it is
  ignored. Leave it unticked otherwise: if the assumption is wrong the error becomes large.
- The legs are not independent. The scale, rotation and shift are common to the whole map,
  and the moments of the controls are searched together, in order. Only the drift
  correction of a leg depends on its own two controls.
