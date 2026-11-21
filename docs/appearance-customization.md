# Appearance customization

## User-facing behavior

- **Background style** selects the existing system, high-contrast dark, or high-contrast light background treatment. The Simplified Chinese label is **背景样式**.
- **Colors** (**配色**) controls the Codex widget mark, quota bars, and percentage numbers independently. The application tray icon and reset text retain their existing rendering.
- Original keeps the upstream defaults. Terracotta, Rose, Glacier, and Lilac provide distinct light/dark-aware combinations. The old `mint` configuration value is accepted as an alias for Rose.
- Classic segmented (tiles) adds small-radius rectangular segments without changing the existing segmented and continuous styles or their defaults.
- **Save current colors as preset…** sits below the three custom-color controls. A named preset appears directly below built-in presets in the same menu. Overwriting or deleting requires a confirmation; cancellation preserves settings. Names are trimmed and compared case-insensitively. There are at most 32 saved presets, each with a 1–40-character name.
- Applying or resetting appearance never erases the saved library. Deleting a saved entry leaves the active colors unchanged.

## Persistence and implementation

`appearance.codex_colors` stores the active base preset and optional `#RRGGBB` overrides. The top-level `color_presets` array stores named copies of that same scheme. Keeping the base preset preserves automatic theme adaptation for colors that have not been overridden.

`codex_colors.rs` owns parsing and color resolution; `user_presets.rs` owns validation, deduplication, limits, and tolerant library loading. Invalid entries are skipped without discarding unrelated settings. `color_picker.rs` uses the native Windows color picker. `preset_dialog.rs` supplies a native modal name dialog with standard Save/Cancel and keyboard behavior. The app-state lock is released before entering any modal dialog.

`provider_icons.rs` recolors the existing Codex artwork with premultiplied alpha and a bounded one-icon cache. Replacing a cached icon releases its old native handle. Original artwork is used unless tinting is requested. `window.rs` connects menu commands, persistence, and rendering; authentication and polling are unchanged.

## Validation

Run `cargo fmt --check`, `cargo test --locked`, and `cargo build --release --locked`.

For native GDI fixtures, set `CODEX_APPEARANCE_PREVIEW_DIR` to a writable directory and run `cargo test --locked render_appearance_previews -- --ignored`. For the interactive name-dialog check, run `cargo test --locked native_dialog_cancel_and_save -- --ignored`, cancel the Chinese dialog, then enter `Evening rose` and save in the English dialog.

Tests cover old settings, saved-scheme round trips, duplicate names, deletion, malformed entries, capacity bounds, custom color independence, native tinting, and the new presets' percentage contrast on the reference light/dark backgrounds. Compare Clippy diagnostics against the unchanged upstream baseline because it already has warnings with current toolchains.
