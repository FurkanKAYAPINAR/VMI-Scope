# Third-party notices

VMI-Scope itself is MIT-licensed (see [LICENSE](LICENSE)). It **embeds** the font
files listed below into the compiled binary. Those files are **not** covered by
VMI-Scope's MIT licence — they keep their own, and their licence texts ship
alongside them in `crates/vmiscope-gui/assets/fonts/`.

The application also reproduces these notices at runtime under
**Settings → About → Licences**.

## Fonts

### Inter

- Version: 4.1 (`InterVariable.ttf`)
- Copyright: © 2016 The Inter Project Authors — <https://github.com/rsms/inter>
- Licence: SIL Open Font License 1.1 — [`LICENSE-Inter-OFL.txt`](crates/vmiscope-gui/assets/fonts/LICENSE-Inter-OFL.txt)

"Inter" is claimed upstream as a reserved name and a trademark of Rasmus
Andersson. The file is embedded unmodified.

### JetBrains Mono NL

- Version: 2.304 (`JetBrainsMonoNL-Regular.ttf`)
- Copyright: © 2020 The JetBrains Mono Project Authors — <https://github.com/JetBrains/JetBrainsMono>
- Licence: SIL Open Font License 1.1 — [`LICENSE-JetBrainsMono-OFL.txt`](crates/vmiscope-gui/assets/fonts/LICENSE-JetBrainsMono-OFL.txt)

The `NL` ("no ligatures") cut is used deliberately: egui 0.35 shapes text with
HarfBuzz defaults and exposes no way to disable OpenType ligatures, so the
standard cut would render `!=` in a WQL filter as a single `≠` glyph. Embedded
unmodified.

### Phosphor Icons

- Version: 2.1.2, regular weight (`Phosphor.ttf`)
- Copyright: © 2020-2021 Phosphor Icons — <https://github.com/phosphor-icons/web>
- Licence: MIT — [`LICENSE-Phosphor-MIT.txt`](crates/vmiscope-gui/assets/fonts/LICENSE-Phosphor-MIT.txt)

Embedded unmodified. The codepoints in `crates/vmiscope-gui/src/theme/icons.rs`
are generated from this exact release and verified against the font's own `cmap`
table; they are not stable across Phosphor major versions.

## A note on modification

All three fonts are embedded **byte-for-byte as released**. Under the OFL FAQ,
subsetting a font counts as modification and would oblige us to rename the
family, so if binary size ever becomes a reason to subset, the renaming
requirement comes with it.

## Rust dependencies

Crate dependencies are declared in `Cargo.toml` / `Cargo.lock` and are checked
for licence compatibility in CI by
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny); the allow-list lives
in [`deny.toml`](deny.toml). Run `cargo deny check licenses` to reproduce.
