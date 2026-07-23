# Icons

Placeholder. Tauri needs `icon.icns` (macOS) referenced in `tauri.conf.json`.

For the first `cargo tauri dev` build, if the icon is missing Tauri will warn
but can fall back to a default. To generate proper icons later from a single
1024x1024 PNG:

```
cargo tauri icon path/to/aygent-logo.png
```

That produces all required sizes incl. `icon.icns`. We'll do the real AYGENT
logo pass during the M1.14 aesthetic milestone.
