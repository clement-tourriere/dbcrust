# Bundled themes

`catppuccin.json`, `tokyonight.json`, `gruvbox.json` and `solarized.json` are
copied unmodified from the theme collection of [GPUI Kit](https://gpui-kit.com)
(`themes/` in <https://github.com/longbridge/gpui-kit>), which is licensed
under the Apache License 2.0. Each file keeps the upstream author and URL of
the color scheme it implements (Catppuccin, Tokyo Night, Gruvbox, Solarized).

They are embedded with `include_str!` in `src/theme.rs` and registered with
GPUI Kit's `ThemeRegistry` at startup. To add a theme, drop a GPUI Kit theme
file here and add it to `BUNDLED_THEMES`.
