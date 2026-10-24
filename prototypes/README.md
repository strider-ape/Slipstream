# UI prototypes (throwaway)

The same classic-table main window (toolbar, download list, segment bar, fake live
progress, follows system light/dark) built twice to pick the GUI toolkit. Not part of
the main workspace, CI or releases.

```
cd prototypes/egui-table && cargo run --release
cd prototypes/slint-table && cargo run --release
SLINT_BACKEND=winit-software cargo run --release   # Slint without OpenGL
```

## Measurements (2026-10-25, WSL2 Ubuntu + WSLg, release builds, after 8 s, animating 10×/s)

| | RAM (RSS) | Private RAM | CPU | Binary |
|---|---|---|---|---|
| egui 0.37 (eframe, OpenGL) | 152 MiB | 78 MiB | 17% | 16.0 MiB |
| Slint 1.18 (OpenGL / femtovg) | 138 MiB | 57 MiB | 22% | 17.3 MiB |
| Slint 1.18, software renderer | **20 MiB** | **3 MiB** | **1%** | 17.3 MiB |

Most of the OpenGL cost is the graphics stack WSLg loads (Mesa + `libLLVM`, 100+ MB);
native Windows/Linux GPU drivers are smaller but still heavy. A table UI needs no GPU, and
Slint can draw on the CPU; egui has no built-in CPU renderer. Binaries use default features
and can be trimmed (e.g. Slint with only the software renderer).

Build notes: egui 0.37 changed its API (`App::ui`, `egui::Panel::top/bottom`); the Slint
version compiled on the first try. First release build: egui 2 min 42 s, Slint 5 min 15 s.
