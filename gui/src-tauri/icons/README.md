# Placeholder icons

These are **derived, not designed**. `docs/oh-ben-claw.png` — the project's own
image, 200×200, RGB, no alpha — resized with Lanczos into the sizes
`tauri.conf.json` asks for.

They exist because nothing here compiled without them. `tauri-build` refuses to
run without `icon.ico`, so `cargo check` in this package could not start, and
between that and the package being excluded from the root workspace, the GUI's
Rust had never been compiled in this checkout. Ten errors' worth of drift were
sitting behind that wall. The icons are the cost of being able to see it, and
the `gui-rust` CI job is what keeps it visible.

## What is wrong with them

- **The source is 200×200.** `128x128@2x.png` is 256×256, so it is upscaled and
  soft. Every other size is a downscale and fine.
- **No alpha channel.** `tray-icon.png` is set `iconAsTemplate: true`, which
  expects a transparent monochrome mask. A full-colour square will render as a
  full-colour square in the tray.
- **No `.icns` care.** Written by Pillow from a single 128px frame rather than a
  proper multi-resolution macOS iconset.

## Replacing them

Supply a 1024×1024 RGBA source and run the tool that exists for this:

    cd gui && npm ci && npx tauri icon path/to/source.png

That regenerates every file in this directory, including a real `.icns` and a
multi-frame `.ico`. Delete this README when it does.
