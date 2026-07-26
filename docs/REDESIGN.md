# VMI-Scope → Nocturne: Implementation Plan

> **Status: planning document.** Nothing here has shipped yet; v0.6.0 is the current
> release. Every egui/eframe API named below was checked against the pinned
> `egui 0.35.0` / `egui_extras 0.35.0` sources rather than recalled, because 0.35
> renamed and removed a great deal (`Rounding` → `CornerRadius`, `SidePanel`/
> `TopBottomPanel` → `Panel`, `Context::set_style` and `Color32::lerp` gone).
> Claims that could not be verified are marked as such — see §5's honesty table
> and §9's privilege caveat.

**Baseline verified against the live repo** (`c:\Users\root\Desktop\Workspace\wmi-scope`, workspace v0.6.0): `crates/vmiscope-gui/src/{app.rs 2973, config.rs 96, main.rs 27}`, core 2717 L across 13 modules. GUI deps today: `eframe/egui/egui_extras 0.35.0`, `rfd 0.17.2`, `serde`, `serde_json` — **no features on `egui_extras`**, no font/theme layer at all.

**Design-source conflict resolved up front:** the shipped DS `styles.css` uses accent `#9184d9` (blurple); the mock `WMI Explorer.dc.html:17` overrides to `--color-accent:#6fa9c6` (steel) with `[data-accent=teal] → #5fb3a5` and `[data-accent=amber] → #c2a05f`. **The mock wins** — steel is the default, teal/amber are the runtime alternates. The DS readme's "freestanding rules fade, in-control separators stay solid" is *contradicted* by `styles.css:262-266`, which explicitly fades `.table tbody tr` rules over 48px at 8% text tint, with hover as a 4% overlay that keeps the rule painting. **The mock/CSS wins: table row rules fade.**

---

## 1. Module layout

```
crates/vmiscope-gui/
  assets/
    fonts/Inter-Regular.ttf            (SIL OFL 1.1)
    fonts/Inter-Medium.ttf             (SIL OFL 1.1)  ← headings, weight 500
    fonts/JetBrainsMono-Regular.ttf    (SIL OFL 1.1)
    fonts/Phosphor.ttf                 (MIT)
    fonts/LICENSE-OFL.txt, LICENSE-MIT-phosphor.txt
    icon.png                           (app icon, 256²)
  src/
    main.rs                    ~70   boot: ViewportBuilder(decorations=false), winit corner pref,
                                     font install, theme install, run_native
    app.rs                    ~300   VmiScopeApp struct + new() + eframe::App::ui:
                                     per-frame preamble, shell assembly, view dispatch,
                                     overlay dispatch, repaint scheduling
    util.rs                   ~140   smart_cmp, toggle_sort, fmt_bytes, fmt_rel_time,
                                     ellipsize_path, is_dangerous_method, save_file
    config.rs                 ~340   (from 96) Config + Accent + Density + SavedQuery{folder,
                                     author,fav,last_ms,last_rows} + Target list + Settings struct
    theme/
      mod.rs                   ~90   Theme{accent,density}, install(ctx), apply_accent(&mut Visuals),
                                     Metrics (density-scaled px), reinstall-on-change
      tokens.rs               ~220   Color32 consts: bg/surface/text/divider, ACCENT_RAMP×3,
                                     NEUTRAL_RAMP, ok/warn/bad, RADIUS_{SM,MD,LG}, SPACE_*
      fonts.rs                ~130   FontDefinitions: "ui"/"ui-med"/"mono"/"icons" families
      icons.rs                ~260   Phosphor PUA codepoint consts (generated, ~120 icons used)
    widgets/
      mod.rs                   ~40
      rule.rs                  ~90   faded_hline / faded_vline via epaint::Mesh (48px ramps)
      table.rs                ~420   DataTable<'a>: columns, sort, virtualized body, row rules,
                                     hover tint, selection, per-cell painters, right-align,
                                     numeric coloring, ellipsis+tooltip
      button.rs               ~170   btn_primary/secondary/ghost/icon, segmented control,
                                     focus_ring helper
      chip.rs                 ~110   tag, filter chip, kind badge (C/A/E), count pill, dot chip
      field.rs                ~150   mono input, filter box w/ leading icon, labelled setting row,
                                     radio group, combo
      card.rs                 ~110   surface card (hairline + ambient shadow), card grid layout
      codeview.rs             ~180   line-numbered mono panel + WQL/PS/C#/VBS/MOF tinting
      kv.rs                    ~90   key/value grid (replaces 4 hand-rolled Grids)
      loading.rs               ~70   spinner + skeleton row + inline "N ms" badge
      export_menu.rs          ~110   Popup::menu export dropdown w/ shortcut hints
    shell/
      mod.rs                   ~30
      chrome.rs               ~170   resize strips (8), drag rect, viewport cmds, maximize state,
                                     corner preference, decorations escape hatch
      titlebar.rs             ~280   40px bar: glyph, title, version pill, machine chip,
                                     palette trigger, live toggle, refresh, 3 window buttons
      rail.rs                 ~180   64px rail: 10 destinations in 3 groups + bottom cluster
      statusbar.rs            ~120   24px bar: live dot, connection, context, prov stats,
                                     shortcut hints, error-log toggle
    state/
      mod.rs                   ~60   AppState re-exports
      ids.rs                   ~90   RequestId, PendingKind, InFlight registry (dedupe + cancel)
      requests.rs             ~280   all request_* (moved from app.rs:476-681)
      responses.rs            ~260   handle_responses (moved from app.rs:720-899)
      explorer.rs             ~200   ns tree, class list, class meta cache, selection, sub-tab
      query.rs                ~140   editor, history, result, sort, timing
      events.rs               ~120   monitor, log ring, filters, stats
      security.rs             ~220   network + persistence + providers state (baselines, sorts)
      machines.rs             ~160   targets, connect form, per-target health   [NEW]
      compare.rs              ~140   A/B selection, diff result                 [NEW]
      errors.rs               ~60    error, error_log, push_error
      search.rs               ~140   index, compute_hits, apply_hit, palette scoring
    views/
      mod.rs                   ~90   View enum (10) + dispatch + per-view title/icon
      explorer/
        mod.rs                ~160   3-column layout + sub-tab strip
        tree.rs               ~180   namespace tree (indent 13px, caret+folder, counts, footer)
        classlist.rs          ~200   filter box + chips + badges + counts + footer
        detail.rs             ~200   breadcrumb, H4 + tags + meta, action row
        instances.rs          ~180   dense sortable table
        properties.rs         ~200   path header + property table w/ per-type icons
        methods.rs            ~170   card grid + Invoke
        schema.rs             ~260   derivation | associations | qualifiers | MOF
        code.rs               ~160   segmented lang + copy/save + line-numbered panel
      query.rs                ~300   editor + gutter + status strip + results + history panel
      events.rs               ~300   config column + live stream w/ flash-in
      saved.rs                ~220   card grid, folders, favourites, import/export
      compare.rs              ~260   A/B pickers, legend, sign column, tinted cells
      machines.rs             ~320   targets table + New-connection panel
      settings.rs             ~280   grouped rows under accent-underlined headings
      network.rs              ~260   RESKIN of app.rs:1851-2026
      persistence.rs          ~340   RESKIN of app.rs:2048-2273
      providers.rs            ~300   RESKIN of app.rs:2279-2451
    overlays/
      mod.rs                   ~40
      invoke.rs               ~320   egui::Modal: signature, params, preview, result, principal
      palette.rs              ~260   Ctrl-K Modal, grouped results, arrow nav
      export.rs                ~60   thin wrapper over widgets::export_menu
      mof.rs                   ~90   MOF viewer (Modal, was Window)
      confirm.rs              ~180   dangerous-method gate
      errorlog.rs              ~90
```

**Migration accounting from today's `app.rs` (2973 L):** ~430 L → `state/requests.rs` + `state/responses.rs`; ~180 L → `state/*` field owners; ~120 L → `util.rs`; ~1150 L → `views/{network,persistence,providers,events}.rs` (reskinned in place); ~700 L → `views/explorer/*` + `views/query.rs` (heavily rewritten); ~250 L → `overlays/*`; ~140 L → `shell/statusbar.rs` + `state/errors.rs`. **Nothing stays in `app.rs` except the struct, `new()`, and `ui()`.**

Total GUI crate after Phase 7: ~8,400 L across 52 files, largest file ~420 L (`widgets/table.rs`).

---

## 2. Theme layer

### 2.1 Token struct

```rust
// theme/tokens.rs
pub const BG:      Color32 = Color32::from_rgb(0x16, 0x18, 0x26);
pub const SURFACE: Color32 = Color32::from_rgb(0x23, 0x25, 0x32);
pub const TEXT:    Color32 = Color32::from_rgb(0xe9, 0xe9, 0xed);
// divider = 16% white. Color32::from_white_alpha is NOT const → premultiplied const equivalent:
pub const DIVIDER: Color32 = Color32::from_rgba_premultiplied(41, 41, 41, 41);
pub const OK:   Color32 = Color32::from_rgb(0x7f, 0xbf, 0x9a);
pub const WARN: Color32 = Color32::from_rgb(0xc9, 0xac, 0x6b);
pub const BAD:  Color32 = Color32::from_rgb(0xcf, 0x8a, 0x84);

pub const NEUTRAL: [Color32; 9] = [ /* #f3f5fe #e4e7f5 #cfd3e5 #b2b6ca #9397ab
                                      #75798c #595d6c #3f424d #292b31 */ ];
pub const STEEL: [Color32; 9] = [ /* #f2f8fb #dfecf3 #bedae8 #95c2d6 #6fa9c6
                                    #4f87a4 #3c6880 #2b4a5c #1d3140 */ ];
pub const TEAL:  [Color32; 9] = [ /* from mock:17-27 */ ];
pub const AMBER: [Color32; 9] = [ /* from mock:32-33 */ ];

// Ramp index helpers so call sites read as design tokens, not magic numbers.
#[inline] pub fn a300(r: &[Color32; 9]) -> Color32 { r[2] }
#[inline] pub fn a500(r: &[Color32; 9]) -> Color32 { r[4] }   // = the accent
#[inline] pub fn a900(r: &[Color32; 9]) -> Color32 { r[8] }

pub const R_SM: CornerRadius = CornerRadius::same(4);
pub const R_MD: CornerRadius = CornerRadius::same(8);
pub const R_LG: CornerRadius = CornerRadius::same(14);

// 0.7× density scale — exact where egui takes f32, rounded where it takes i8.
pub const S1: f32 = 2.8; pub const S2: f32 = 5.6;  pub const S3: f32 = 8.4;
pub const S4: f32 = 11.2; pub const S6: f32 = 16.8; pub const S8: f32 = 22.4;
```

```rust
// theme/mod.rs
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Accent { Steel, Teal, Amber }
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Density { Compact, Comfortable }

pub struct Theme { pub accent: Accent, pub density: Density }

/// Density-derived pixel metrics. Everything the views measure comes from here —
/// no view is allowed to write a raw px literal.
pub struct Metrics {
    pub row_h: f32, pub header_h: f32, pub rail_item_h: f32,
    pub tree_indent: f32, pub gutter_w: f32, pub card_min_w: f32,
    pub s1: f32, pub s2: f32, pub s3: f32, pub s4: f32, pub s6: f32, pub s8: f32,
}
impl Metrics {
    pub fn for_density(d: Density) -> Self {
        let k = match d { Density::Compact => 1.0, Density::Comfortable => 1.30 };
        Self { row_h: 21.0 * k, header_h: 22.0, tree_indent: 13.0, /* … */ }
    }
}
```

### 2.2 Installation + accent swap

Install through `ctx.all_styles_mut` (context.rs:2145) so an OS light-theme flip can never expose stock egui colors, then pin `set_theme(ThemePreference::Dark)`. **`Context::set_style` does not exist in 0.35** — that call will not compile.

```rust
pub fn install(ctx: &egui::Context, theme: Theme) {
    let ramp = theme.ramp();                  // &'static [Color32; 9]
    let m = Metrics::for_density(theme.density);
    ctx.all_styles_mut(|s| {
        s.visuals = base_visuals();           // bg/surface/text/divider/status/radii/shadows
        apply_accent(&mut s.visuals, ramp);   // the fan-out — see below
        s.text_styles = text_styles();
        s.spacing.item_spacing   = egui::vec2(m.s2, m.s1);
        s.spacing.button_padding = egui::vec2(m.s3, m.s1);
        s.spacing.interact_size  = egui::vec2(0.0, m.row_h);
        s.spacing.indent         = m.s6;
        s.spacing.menu_spacing   = m.s1;
        s.spacing.window_margin  = egui::Margin::same(8);   // 8.4 → 8 (i8)
        s.spacing.menu_margin    = egui::Margin::symmetric(6, 3);
        s.spacing.scroll = egui::style::ScrollStyle {
            floating: false, bar_width: 8.0, handle_min_length: 22.4,
            bar_inner_margin: 3, bar_outer_margin: 0, ..egui::style::ScrollStyle::solid()
        };
        s.spacing.scroll.fade.strength = 0.0;
    });
    ctx.set_theme(egui::ThemePreference::Dark);
}

/// egui has NO single accent field. It must be fanned to six places or the
/// accent swap will visibly half-apply.
pub fn apply_accent(v: &mut egui::Visuals, r: &[Color32; 9]) {
    let a = a500(r);
    v.selection.bg_fill        = a.gamma_multiply(0.30);
    v.selection.stroke         = egui::Stroke::new(1.0, TEXT);
    v.hyperlink_color          = a;
    v.text_cursor.stroke       = egui::Stroke::new(1.0, a);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, a);
    v.widgets.active.bg_stroke  = egui::Stroke::new(1.0, a);
    v.widgets.active.bg_fill    = a.gamma_multiply(0.22);
    v.widgets.active.weak_bg_fill = a.gamma_multiply(0.22);
}
```

Accent swap at runtime = `theme.accent = new; theme::install(ctx, theme); config.save();` — one call, one frame. Density swap re-runs the same path and rebuilds `Metrics`. **Do not** implement density via `Context::set_zoom_factor` — that scales fonts too and breaks the 13px body / 9px rail-label relationship.

`Visuals::striped = true` + `faint_bg_color`: **leave `striped` false**. The design has no zebra, only row rules + 4% hover. Setting `faint_bg_color` to an opaque surface would be a real behavior change (egui's default is *additive* white α0, `style.rs:1504`).

### 2.3 Fonts

> **Correction to an earlier draft of this plan: egui 0.35 does not use `ab_glyph`.**
> `epaint 0.35` depends on **skrifa 0.42** (rasterization) + **harfrust 0.7** (OpenType
> shaping) + read-fonts/font-types. Two consequences drive every decision below.

**Consequence 1 — variable fonts are supported, so weights are free.** `epaint`
exposes `FontTweak { coords: VariationCoords, .. }` and builds a
`harfrust::ShaperInstance::from_variations(..)` — real instancing, not faux-bold.
Register the *same* `&'static [u8]` twice under two names with different `wght`
coords: `include_bytes!` emits the blob once, so weight 500 costs zero extra bytes.

**Consequence 2 — ligatures now fire and cannot be switched off.** egui ≤ 0.34 did
no shaping, so ligatures never rendered. harfrust applies HarfBuzz's default feature
set (`liga`, `calt`, `clig`, `rlig`, `kern`) and `epaint` exposes **no** OpenType
feature control. In a tool that renders paths, WQL and hex, stock JetBrains Mono
would silently ligate `!=` → `≠`, `->` → `→`, `::`, `==`. **Use the `NL`
("No Ligatures") build** — which is also 65 KB smaller. This is the single most
likely thing to bite on an egui upgrade.

| File | Bytes | Family slot | Licence | Source (pinned) |
|---|---|---|---|---|
| `InterVariable.ttf` | 879,708 | `Proportional[0]` `Name("ui")` @ `wght 400`, and `Name("ui-med")` @ `wght 500` | SIL OFL 1.1 | `rsms/inter` **v4.1**, zip root |
| `JetBrainsMonoNL-Regular.ttf` | 208,576 | `Monospace[0]`, `Name("mono")` | SIL OFL 1.1 | `JetBrains/JetBrainsMono` **v2.304**, `fonts/ttf/` |
| `Phosphor.ttf` | 488,636 | fallback in **both** families | MIT | `phosphor-icons/web` **v2.1.2**, `src/regular/` |

Total 1,576,920 B of font data + 9,855 B of licence texts. `InterVariable` carries
`wght 100–900` + `opsz 14–32`; `JetBrainsMono[wght]` carries `wght 100–800`.

**Binary size — measured, and better than it looks.** Font bytes pass through at
1:1 (`strip=true` removes symbols, not `.rdata`; thin LTO never touches an opaque
byte array). But `epaint_default_fonts` already embeds **1,414,020 B** into every
egui binary (Hack + NotoEmoji + Ubuntu-Light + emoji-icon-font), all of which
becomes dead weight once we ship our own faces. Disable it:

```toml
eframe = { version = "0.35.0", default-features = false, features = [
    "accesskit", "wgpu", "winit/default",
] }   # "default_fonts" deliberately omitted
```

**Net binary change: +162,900 B (+159 KiB).** Forgetting to drop `default_fonts` is
the actual risk, not embedding fonts. Caveat: dropping it also drops the emoji
fallback — acceptable here precisely because the redesign replaces every emoji
literal (`🔍`, `⚙`, `📄`) with a Phosphor glyph, but it means an emoji arriving in
*WMI data* renders as tofu.

Licence obligations: OFL-1.1 §2 permits bundling in software (including sold
software) provided the copyright notice + full licence text travel with every copy;
§5 keeps the font files under OFL — a repo-root MIT `LICENSE` must not read as
covering `assets/fonts/`. §3: **subsetting counts as modification under the OFL FAQ,
so a subset must be renamed** — ship the fonts unmodified. Phosphor's MIT notice
names a different copyright holder and must be reproduced separately. All three are
discharged by `THIRD-PARTY-NOTICES.md` + the bundled licence files + the
Settings → About → Licences panel (task 7.4). **Do not** use the
`@import url('https://fonts.googleapis.com/…')` from `styles.css:2` — a security
tool must not fetch fonts at runtime.

Heading weight: egui's `FontId` is only `{size, family}` — there is no weight axis on
`FontId`, and `RichText::strong()` recolors rather than emboldens. The weight comes
from the *variation coords* on the registered family (`ui-med` @ `wght 500`), which
is why `strong()` stays banned (invariant I3).

```rust
// theme/fonts.rs
const INTER:    &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");
const JBMONO:   &[u8] = include_bytes!("../../assets/fonts/JetBrainsMonoNL-Regular.ttf");
const PHOSPHOR: &[u8] = include_bytes!("../../assets/fonts/Phosphor.ttf");

/// One blob, two weights — `include_bytes!` emits INTER once.
fn inter_at(weight: f32) -> FontData {
    FontData::from_static(INTER).tweak(FontTweak {
        coords: VariationCoords::new([(b"wght", weight), (b"opsz", 14.0)]),
        ..Default::default()
    })
}

pub fn install(ctx: &egui::Context) {
    // `default_fonts` is off, so this map starts EMPTY — every built-in family
    // must be populated or `TextStyle::resolve` panics.
    let mut f = egui::FontDefinitions::empty();
    f.font_data.insert("ui".into(),     Arc::new(inter_at(400.0)));
    f.font_data.insert("ui-med".into(), Arc::new(inter_at(500.0)));
    f.font_data.insert("mono".into(),   Arc::new(FontData::from_static(JBMONO)));
    f.font_data.insert("icons".into(),  Arc::new(FontData::from_static(PHOSPHOR)));

    // Icon font goes AFTER the text font — fallback is per-character, first match
    // wins, and Phosphor ships its icon NAMES as ligatures ("copy", "key", "star",
    // "folder"). If it ever led a family, those words would render as icons.
    f.families.insert(FontFamily::Proportional,          vec!["ui".into(),     "icons".into()]);
    f.families.insert(FontFamily::Monospace,             vec!["mono".into(),   "icons".into()]);
    f.families.insert(FontFamily::Name("ui-med".into()), vec!["ui-med".into(), "icons".into(), "ui".into()]);

    ctx.set_fonts(f);   // once, at startup — set_fonts does a full TTF byte compare per call
}

pub fn text_styles() -> BTreeMap<TextStyle, FontId> {
    // TextStyle::resolve PANICS on a missing key — all five built-ins are mandatory.
    [ (TextStyle::Small,     FontId::new(11.0, FontFamily::Proportional)),
      (TextStyle::Body,      FontId::new(13.0, FontFamily::Proportional)),
      (TextStyle::Button,    FontId::new(12.0, FontFamily::Proportional)),
      (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
      (TextStyle::Heading,   FontId::new(19.0, FontFamily::Name("ui-med".into()))),
      (TextStyle::Name("rail".into()),   FontId::new(9.0,  FontFamily::Proportional)),
      (TextStyle::Name("caption".into()),FontId::new(10.5, FontFamily::Proportional)),
      (TextStyle::Name("th".into()),     FontId::new(11.0, FontFamily::Proportional)),
      (TextStyle::Name("code".into()),   FontId::new(11.5, FontFamily::Monospace)),
    ].into()
}
```

Icon registration is via `FontDefinitions` (not `Context::add_font`) because we are already rebuilding the map for the two text families; `add_font` dedupes by name only and would be a second full font rebuild.

`icons.rs` is **generated once** from `phosphor-icons/web@2.1.2/src/regular/style.css`
by a throwaway script and checked in as `pub const TREE_STRUCTURE: &str = "\u{e67c}";`
etc. — only the glyphs the design uses. Codepoints live in the BMP Private Use Area;
the v2.1.2 font occupies `U+E000..=U+EE83` with 1,513 PUA glyphs. **Codepoints are not
stable across Phosphor major versions — pin v2.1.2.**

The 54 codepoints the mock needs were verified three ways with zero disagreements:
`style.css` (1,530 rules) vs `selection.json` (1,512 icons) vs the actual `cmap`
table of `Phosphor.ttf` — all 54 resolve to real glyph IDs. A wrong codepoint renders
as a blank box, so the generator must never guess: any name that does not resolve is
a hard error, not a fallback. Add a `#[test]` asserting every const is a single char
in `U+E000..=U+F8FF`.

### 2.4 Tokens egui cannot express — and the fallback

| Token | egui status | Fallback |
|---|---|---|
| **48px fading row/section rules** | No gradient on `Frame` or `RectShape`; `Frame.fill` is a flat `Color32` | `widgets::rule::faded_hline(painter, rect, color)` builds an 8-vertex `epaint::Mesh` (3 quads: α0→α, solid, α→α0) via `colored_vertex` + `add_triangle`, pushed with `Painter::add(Shape::mesh(m))`. ~6 tri/rule × ~40 visible rows = negligible. **Never mix with textured verts** (`colored_vertex` debug-asserts `TextureId::default()`). |
| **Per-cell tinted backgrounds (Compare)** | egui_extras has no per-cell fill; only `striped`/`selected`/`hovered`, all fixed to theme slots | First statement in the cell closure: `let g = ui.max_rect().expand2(0.5*ui.spacing().item_spacing).round_ui(); ui.painter().rect_filled(g, 0, tint);` — `round_ui` needs `use egui::emath::GuiRounding as _`. **`clip_rect_margin` defaults to 0.0 in 0.35**, so on a `.clip(true)` column anything painted outside `max_rect` is *discarded entirely*. Compare's value columns must be **unclipped** (or the tint must be inset). |
| **Row hover = 4% text tint** | egui_extras hover uses `widgets.hovered.bg_fill`, is **one frame late**, and requires an interactive `Sense` | `body.ui_mut().visuals_mut().widgets.hovered.bg_fill = Color32::from_white_alpha(10)` (4% of 255 ≈ 10) + `.sense(Sense::click())` on the builder. Accept the 1-frame lag. |
| **Focus ring: 2px accent, offset 2** | No focus ring concept; `Widgets` has **no `focused` state** — focus reuses `active` | `widgets::button::focus_ring(ui, &resp)`: `if resp.has_focus() { ui.painter().rect_stroke(resp.rect.expand(2.0), R_SM, Stroke::new(2.0, accent), StrokeKind::Outside) }`. Call it after every interactive widget in the kit. |
| **Fractional spacing in margins** | `Margin` is four `i8`; `From<f32>` rounds | 2.8→3, 5.6→6, 8.4→8, 11.2→11, 16.8→17, 22.4→22 in `window_margin`/`menu_margin`/`Frame::inner_margin`. Exact fractions survive only in `item_spacing`, `button_padding`, `interact_size`, `indent`, `menu_spacing`, `icon_spacing`. Where exactness matters visually, use `ui.add_space(8.4)` instead of a margin. |
| **`ui.separator()` spacing** | `Style::separator_style` hard-codes `spacing: 6.0` and is a **method, not a field** | Ban `ui.separator()` project-wide; use `widgets::rule::*`. |
| **Table row-rule color independent of column-resize separator color** | `set_overline` and the resize separator both read `widgets.noninteractive.bg_stroke` | Do not use `set_overline`. Paint the faded rule ourselves at row bottom inside the first cell (unclipped marker column) — which we need anyway for the 48px fade. |
| **Scrollbar color** | `ScrollStyle` has **zero color fields** | Tune `extreme_bg_color` (trough) + `widgets.{inactive,hovered,active}.bg_fill` (handle) to land on-token. |
| **Line-height / letter-spacing globally** | `FontId` = `{size, family}` only; no line-height in `Style` | Rhythm via `interact_size.y` + `item_spacing.y`. Per-run letter-spacing exists: `RichText::extra_letter_spacing(0.5)` — used for the 11px uppercase table headers and the 9px rail labels. |
| **Rounded window corners** | No `ViewportBuilder`/`ViewportCommand` API | `cc.winit_window()` → `WindowExtWindows::set_corner_preference(CornerPreference::Round)`. Requires adding `winit = "0.30.13"` (unifies with eframe's pin). **Not** raw DWM FFI. |
| **Window transparency** | Effectively unsupported on Windows/wgpu | Design for an opaque shell. Do not request `with_transparent`. |
| **`egui::hex_color!`** | Gated behind the `color-hex` feature, not enabled | `Color32::from_rgb(0x16,0x18,0x26)` (const). |
| **Points vs px** | egui units are points; HiDPI scales them | Accept. Document that 13px body = 13pt at 100% scale. Do **not** pin `set_pixels_per_point`. |

---

## 3. Shell

### 3.1 Boot

```rust
// main.rs
let native_options = eframe::NativeOptions {
    viewport: egui::ViewportBuilder::default()
        .with_title("VMI-Scope")
        .with_decorations(cfg.decorated)          // default false
        .with_resizable(true)
        .with_inner_size([1240.0, 780.0])
        .with_min_inner_size([980.0, 560.0])      // raised: 3-col explorer needs 980
        .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))?),
    ..Default::default()
};
```

In `VmiScopeApp::new(cc)`:
```rust
theme::fonts::install(&cc.egui_ctx);
theme::install(&cc.egui_ctx, Theme { accent: cfg.accent, density: cfg.density });
#[cfg(windows)]
if !cfg.decorated {
    use winit::platform::windows::{CornerPreference, WindowExtWindows};
    if let Some(w) = cc.winit_window() { w.set_corner_preference(CornerPreference::Round); }
}
```

### 3.2 Panel stack (all inside `eframe::App::ui`, which is mandatory in 0.35)

`SidePanel`/`TopBottomPanel` **do not exist**; there is one unified `egui::Panel` with `::top/::bottom/::left/::right`, and `show` takes only `&mut Ui`.

```
CentralPanel::default().frame(shell_frame)          // outer: BG fill + 1px DIVIDER stroke, R_LG
 ├ Panel::top("vs_titlebar").exact_size(40.0).resizable(false).show_separator_line(false)
 │     .frame(Frame::NONE.inner_margin(Margin::symmetric(10,0)).fill(TITLEBAR_BG))
 ├ Panel::bottom("vs_status").exact_size(24.0).resizable(false).show_separator_line(false)
 ├ Panel::left("vs_rail").exact_size(64.0).resizable(false).show_separator_line(false)
 └ CentralPanel::default().frame(Frame::NONE.fill(BG))   ← per-view content, added LAST
```

Critical mechanics, all verified:

- **`exact_size` is the OUTER size including margins/stroke.** The default panel `Frame::side_top_panel` adds `Margin::symmetric(8,2)`, which would leave 36px of a 40px bar. Every chrome panel gets an explicit `Frame::NONE.inner_margin(…)`.
- **`show_separator_line(false)` alone is not enough.** The separator is a `painter().hline/vline` drawn by the *parent* Ui, and the `is_resizing`/`resize_hover` branches take precedence over the flag. Those are only forced false when `resizable == false`. **Always pair both.** Then paint our own 1px `DIVIDER` hline/vline at `bottom()-0.5` / `right()-0.5` inside each panel's closure so it is clipped and layered above the panel fill.
- **`Panel::left/right` default to `resizable: true`** (only `top`/`bottom` are constructed non-resizable). The 64px rail **must** call `.resizable(false)` explicitly.
- Panel ids persist per id (`PanelState` in `ctx.data`), so ids must be unique and stable: `vs_titlebar`, `vs_rail`, `vs_status`, `vs_ns_tree`, `vs_class_list`, `vs_history`, `vs_events_cfg`, `vs_new_conn`.

### 3.3 Title bar

Ordering is load-bearing: **egui's hit test picks the last-registered overlapping widget.** So the drag rect is registered **before** `Panel::top(...).show(...)`, and every button inside the panel wins the tie.

```rust
let shell = ui.max_rect();
let bar_rect = Rect::from_min_size(shell.min, vec2(shell.width(), 40.0));
let bar = ui.interact(bar_rect, Id::new("vs_titlebar_drag"), Sense::click_and_drag());
let is_max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
if bar.double_clicked() {                                   // check BEFORE drag
    ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!is_max));
} else if bar.drag_started_by(PointerButton::Primary) {
    ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
}
// ...then Panel::top("vs_titlebar").show(ui, |ui| { glyph, title, pill, chip, palette, ... })
```

Contents, left→right: 22px accent-outlined app glyph (`Frame` with `R_SM` + 1px accent stroke, Phosphor `ph-tree-structure` at 13px in accent) · "WMI Explorer" (Heading family, 13.5px) · version pill (`v0.7.0`, mono 10px, 8% text bg, `R_SM`) · machine chip (dot colored by `ConnStatus` + `\\HOST` mono + 1px divider vline + transport text + caret; click → `View::Machines`) · **spacer** · palette trigger box (`ph-magnifying-glass` + "Search classes, properties, commands" + "Ctrl K" pill; click → `palette_open = true`) · **spacer** · Live/Paused ghost toggle (`ph-pulse`, pulsing when live) · Refresh ghost button (`ph-arrows-clockwise`) · three `38×40` frameless buttons.

There is **no toggle-maximize command** — read `viewport().maximized` and send the negation. Window buttons:

```rust
if ui.add(Button::new(ICON_MINUS).frame(false).min_size(vec2(38.0,40.0))).clicked() {
    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
}
// maximize icon swaps on is_max: ph-square ↔ ph-corners-in
// close: hover fill = BAD.gamma_multiply(0.30); ViewportCommand::Close
```

`ViewportCommand::Close` on the root viewport shuts the app down unless answered with `CancelClose` — we don't need a gate, but the invoke-in-flight case is worth one (see task 7.9).

### 3.4 Rail (final order — resolves the 7-slot problem)

The mock has 7; we ship **11 destinations in 3 groups**, separated by faded 40px-wide hairlines, with the settings/help/avatar cluster pinned to the bottom. A 64px × 11-item rail at 44px/item = 484px, which fits the 560px min content height (780 − 40 − 24 = 716 at default size).

```
┌── EXPLORE ──────────────┐
│ 1  ph-tree-structure    Explorer     │
│ 2  ph-terminal-window   Query        │
│ 3  ph-broadcast         Events       │   ← live event stream (today's "Events" tab)
├── SECURITY ─────────────┤
│ 4  ph-cpu               Process      │   ← NEW, §9: live start/stop trace, ended rows stay dim
│ 5  ph-globe-hemisphere  Network      │
│ 6  ph-shield-warning    Persist      │   ← today's "Persistence"; label truncated for 64px
│ 7  ph-plugs-connected   Providers    │
├── DATA ─────────────────┤
│ 8  ph-bookmark-simple   Saved        │
│ 9  ph-git-diff          Compare      │
│ 10 ph-desktop-tower     Machines     │
└─────────────────────────┘
   (spacer)
│ 11 ph-gear-six          Settings     │   ← moved out of the mock's inline slot
│    ph-question          (help)       │
│    (avatar circle)                   │
```

`Process` leads the security group deliberately: process start/stop is the telemetry
the other three views get correlated *against* (which process opened that socket,
which process wrote that subscription, which process hosts that provider).

Rationale: the design's three deliberate clusters (browse / library / config) survive; our three security views become a coherent *third* story rather than being smuggled in at the end. Selected item = accent fg + `accent.gamma_multiply(0.15)` background pill at `R_MD`, inset 4px. `Persistence` is renamed **"Persist"** in the 9px rail label only (11 chars at 9px Inter ≈ 50px, marginal in a 56px usable width) — the view heading, palette entry and status-bar context all still say "Persistence".

### 3.5 Status bar

24px, `Frame::NONE.fill(SURFACE.gamma_multiply(0.55) over BG).inner_margin(Margin::symmetric(12,0))`, 11px Small. Left: live dot (pulsing) + connection text. `·` + view context text. Right (`Layout::right_to_left`): "F5 refresh" · "Ctrl K palette" · provider-host stats (mono; dashes when the poll is disabled) · error-log toggle `Log (n)` when `error_log` is non-empty (preserved from today's `ui_status:2779`).

### 3.6 Window resizing (mandatory re-implementation)

With `decorations(false)`, egui-winit forwards `with_undecorated_shadow(true)`, winit's `WM_NCCALCSIZE` swallows the whole non-client area, and winit implements **no `WM_NCHITTEST`**. So: no OS resize border, no OS caption drag, and a documented **1px black line at the top edge** (`params.rgrc[0].top += 1`).

`shell/chrome.rs::resize_strips(ui)` registers 8 strips (4 edges 6px + 4 corners 6×6) **after every panel**, since hit test picks last:

```rust
for (rect, dir, cursor) in strips(ui.max_rect(), 6.0) {
    let r = ui.interact(rect, Id::new(("vs_resize", dir as u8)), Sense::drag());
    if r.hovered() || r.dragged() { ui.ctx().set_cursor_icon(cursor); }
    if r.drag_started() { ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir)); }
}
```

Aero-snap and `Win`+arrow still work because winit keeps `WS_SIZEBOX` on undecorated resizable windows.

### 3.7 What breaks with decorations, and the escape hatch

We ship `decorations = false`. But add `Config.decorated: bool` (+ `--decorated` CLI flag), because custom chrome costs three things that cannot be recovered in egui 0.35:

1. **Windows 11 Snap Layouts** — the flyout that appears when hovering the OS maximize button requires answering `WM_NCHITTEST` with `HTMAXBUTTON`. winit does not handle `WM_NCHITTEST` at all and eframe does not expose a message hook. **Permanently lost** under custom chrome.
2. **Screen-reader / AT window-controls discovery** — AccessKit exposes our buttons as generic buttons, not as system caption buttons.
3. **Tiling/remote-desktop edge cases** — some RDP and third-party window managers rely on the real caption.

When `decorated == true`: `shell/titlebar.rs` renders the **same 40px bar minus the three window buttons**, skips the drag rect and resize strips, and skips `set_corner_preference`. That is one `if` at the top of `titlebar::show` and one in `chrome::resize_strips`. Everything else (rail, status bar, views) is identical. This must be a *tested* path, not an aspiration — it is the accessibility fallback and the bug-report escape hatch.

---

## 4. Per-view plan

| # | View | Verdict | Data it needs | Core today? |
|---|---|---|---|---|
| 1 | **Explorer** | **MERGE + rebuild.** Absorbs today's `ui_namespace_tree`, `ui_class_list`, `ui_central`, `ui_schema`, `ui_detail`, `ui_script_gen`, `ui_search` into 3 columns + 5 sub-tabs. Query editing *leaves* for view 2. | ns children ✔ · class list ✔ · **per-ns class count** ✖ · **per-class instance count** ✖ · **class kind badge C/A/E** ✖ · schema props/methods ✔ · **class qualifiers (Dynamic, Provider)** ✖ (read then discarded) · **`__Derivation`** ✖ · **associations** ✖ · MOF ✔ · instances ✔ · **elapsed ms** ✖ | 4/11 |
| 1a | Instances sub-tab | RESKIN of `ui_central:1163-1232` | `QueryResult` from `SELECT * FROM <class>` ✔ | ✔ |
| 1b | Properties sub-tab | RESKIN of `ui_schema:1320-1367` + per-property icons (key/writable/numeric/string/datetime) — all derivable from `PropertySchema{is_key,is_write,cim_type}` ✔ | + **Qualifiers column** ✖ | partial |
| 1c | Methods sub-tab | **NEW** card grid (was a flat list). `MethodSchema{name,is_static,in_params,out_params,description}` ✔ | `is_static` ✔ but unreliable when the `Static` qualifier is absent | ✔ (caveat) |
| 1d | Schema sub-tab | **NEW** derivation + associations + qualifiers; MOF panel is a RESKIN of `ui_mof_window` | `__Derivation` ✖, `REFERENCES OF … WHERE SchemaOnly` ✖, class qualifier map ✖ | ✖ |
| 1e | Code sub-tab | **MERGE** of `ui_script_gen:1236-1260`, extended PowerShell/VbScript → **+ C#, + WQL** | `generate_script` ✔ (needs 2 new arms) | ✔ |
| 2 | **Query** | **MERGE + reskin.** Today's query editor + result table + `config.history` promoted from a sliver of `ui_central` to a full view with a 262px History panel. | WQL ✔ · results ✔ · **elapsed ms** ✖ · row count ✔ · "ORDER BY is local" note = derive by scanning the WQL string (easy, no core change) | mostly ✔ |
| 3 | **Events** | **RESKIN** of `ui_events:2597-2673`. Adds: config column, `WITHIN` field, Delivery segmented, stats card, kind badges, accent flash-in. | `EventMonitor` ✔ · **delivery rate** = computable client-side ✔ · **"Sink queue"** ✖ (not observable — replace with real mpsc backlog or drop) · **Permanent delivery** ✖ (would write `__EventFilter`/consumer/binding) · **remote monitoring** ✖ (`monitor.rs:87` is local-only) | partial |
| 4 | **Saved** | **MERGE + extend** of `ui_save_query_window` + `config.saved`. New: card grid, favourites, folders, author, last ms/rows. | `SavedQuery{name,namespace,wql}` ✔ — needs `+folder,+fav,+author,+last_ms,+last_rows`. `namespace` is stored today but **never read back** (`app.rs:1142-1145` restores only `wql`) — fix that. | ✔ w/ schema bump |
| 5 | **Compare** | **NEW.** | Two labelled `QueryResult`s + a keyed row diff | ✖ — three structural blockers (§5.10) |
| 6 | **Machines** | **NEW** view; **MERGES** today's `ui_connection_bar:2679-2763` (host/creds/connect), which is deleted from the top bar. | target list (new config) · transport ✔(DCOM only) · credential ✔ · **RTT** ✖ · **OS build** ✖ · **status** ✖ | partial |
| 7 | **Settings** | **NEW** view; **MERGES** `global_theme_preference_switch` (deleted) + all today's implicit constants. | `Config` extension only | ✔ (pure GUI) |
| 8 | **Network** | **RESKIN** of `ui_network:1851-2026` — same data, new table kit, fade-on-close preserved via `gamma_multiply(alpha)` per row. | `NetworkSnapshot` ✔, `is_external` ✔, `state_color` → token ramp | ✔ |
| 9 | **Persistence** | **RESKIN** of `ui_persistence:2048-2273` — risk pills from the status tokens, faded rules, `CollapsingHeader` diff → card. Export CSV/JSON/HTML + snapshot/baseline preserved. | `SubscriptionReport` ✔, `diff_subscriptions` ✔, `subscriptions_to_html` ✔ | ✔ |
| 10 | **Providers** | **RESKIN** of `ui_providers:2279-2451` + **extend** with host-process CPU/memory (feeds the status bar). | `ProviderInfo` ✔ · **cpu/mem/handles/threads/quota** ✖ | partial |
| 11 | **Process** | **NEW** (§9). Live process start/stop with ended rows retained and dimmed. Not in the mock — derived from a competitive analysis of `WMIProcessWatcher.exe` plus a measured gap in our own event monitor. | `Win32_ProcessStartTrace`/`StopTrace` ✖ · elevation probe ✖ · SID→user ✖ · best-effort `CommandLine` ✖ | ✖ |

**Overlays:** Invoke = MERGE of `ui_actions:1445-1642` + `ui_confirm_window:1644-1764` into one `egui::Modal` with the danger gate inline. Palette = NEW, but reuses `compute_hits:2457` + `apply_search_hit:2506` verbatim. Export dropdown = NEW (`Popup::menu`), replacing 4 inline export button clusters. MOF/Error-log = RESKIN `Window` → `Modal`.

---

## 5. Gap list — design element → concrete core work

Feasibility: **E** easy (pure Rust / existing plumbing) · **R** needs raw COM (`windows 0.62` direct, outside the `wmi` crate) · **X** expensive at runtime · **N** not possible.

| # | Design element | Concrete call | Rating |
|---|---|---|---|
| 5.1 | **Query elapsed ms** ("Completed in 412 ms", "61 ms", card meta) | `Instant::now()` around `q_maps` in `worker::run_query`; add `elapsed_ms: u64` to `QueryResult` and to `Classes`/`ChildNamespaces`/`Schema`/`SearchIndex`/`Providers`/`Network`/`EventSubscriptions`/`HostConnected`. Split connect vs execute — `connect()` runs **per request** (`worker.rs:457`) so its cost is currently folded into every query. | **E** |
| 5.2 | **Class kind badges C / A / E + filter chips** | Same `get_object` round trip as the schema read. Dynamic = class qualifier `Dynamic`; Association = qualifier `Association`; Event = `__Derivation` contains `__Event`; System = name starts `__`; Abstract = `Abstract` ✔ already read. Add `ClassKind` bitflags to `ClassSchema` + a `Vec<ClassBrief{name,kind,provider}>` `Response::Classes` variant. | **E** for one class; **X** for a whole namespace (1,400 classes × a `get_object` each in `root\CIMV2`) → must be **lazy + streamed + cached**, never eager |
| 5.3 | **Class qualifiers panel (Dynamic / Provider / UUID / Singleton / Supports*)** | `reflect.rs:155-167` **already calls** `GetQualifierSet()` and `read_qualifiers()` returns every qualifier as `(String, Variant)` — the `match` then throws all but `description`/`abstract` away. Widen it; store `Vec<(String,String)>` on `ClassSchema`. | **E** — pure deletion of a filter |
| 5.4 | **Derivation chain ("derives from CIM_Process", Schema ancestry)** | `wrapper.get_property("__Derivation")` → `Variant::Array` → `value::variant_to_string_vec`. **Trap:** `list_properties()` passes `WBEM_FLAG_NONSYSTEM_ONLY`, so `__Derivation` never appears in enumeration — it must be fetched **by name**. Same for `__Genus`, `__Dynasty`, `__Property_Count`. Zero extra COM round trips (same object). | **E** |
| 5.5 | **Associations panel** | `ExecQuery("WQL", "REFERENCES OF {Win32_Process} WHERE SchemaOnly", …)` for association *classes*; `ASSOCIATORS OF {…} WHERE SchemaOnly` for endpoint classes. **Do not** use `wmi::WMIConnection::associators::<R,A>` — it derives class names from Rust type names and needs `DeserializeOwned`. Go through `exec_query(&str)`. **Blocked on 5.10** — results are only useful once `__PATH`/`__CLASS` survive. | **R** |
| 5.6 | **Per-class instance count** | WQL has **no `COUNT(*)`**. Only: `ExecQuery("SELECT __RELPATH FROM <C>", WBEM_FLAG_FORWARD_ONLY \| WBEM_FLAG_RETURN_IMMEDIATELY)` then `IEnumWbemClassObject::Next(timeout, &mut [None;64], &mut n)` summing `n` and dropping objects. Or `CreateInstanceEnum(class, WBEM_FLAG_SHALLOW\|FORWARD_ONLY\|RETURN_IMMEDIATELY)` (shallow = exclude subclasses; both counts are useful). All present in `windows 0.62.2`. | **R + X** — unbounded work per class. **Must** be cancellable, per-class budgeted (finite `lTimeout` per `Next`), skip abstract/association/event classes, and never auto-run for a whole namespace. `CIM_DataFile` will effectively never finish. |
| 5.7 | **Namespace class counts in the tree** | `CreateClassEnum(&BSTR::new(), WBEM_FLAG_DEEP\|FORWARD_ONLY\|RETURN_IMMEDIATELY)` counted without reading `__CLASS` — meaningfully cheaper than today's `SELECT * FROM meta_class` + per-object `obj.class()`. Recursive rollup = DFS over `SELECT Name FROM __NAMESPACE`; **no server-side rollup exists**. New `Request::NamespaceStats{id, namespace, recursive}`. | **R + X** |
| 5.8 | **Static vs instance methods** | `MethodSchema.is_static` ✔ already read from the `Static` method qualifier (`reflect.rs:253`) and honored at `method.rs:142`. **Caveat:** the qualifier is often absent → a genuinely-static method renders as instance-only. Robust classifier also treats "class has no `Key` property" and `Singleton == TRUE` as static-capable, and falls back to retrying the class-path invocation on `WBEM_E_INVALID_METHOD` (0x8004102E — measured; the plan originally guessed `WBEM_E_ILLEGAL_OPERATION`). Also missing: `Implemented` qualifier, per-param `IN`/`OUT` (params in both lists are duplicated with no marker; genuine `[in,out]` parameters are rare — 3 in `root\CIMV2`, all on `*USBHub/*USBDevice.GetDescriptor`). | **E** (improvement, not new plumbing) |
| 5.9 | **Machines: connect RTT + OS build + status dot** | RTT = `Instant` around the existing `SetHost` probe (`worker.rs:422` runs `SELECT Name FROM Win32_ComputerSystem` and **throws the result away**). OS = `SELECT Caption, Version, BuildNumber, OSArchitecture, LastBootUpTime FROM Win32_OperatingSystem`. Machine identity = `SELECT UUID FROM Win32_ComputerSystemProduct` (the stable join key for Compare). New shape: `Response::HostConnected{id, host, connect_ms, probe_ms, os: HostInfo}`. **UBR/patch revision is not in WMI** — needs `root\default` `StdRegProv.GetDWORDValue(HKLM, "SOFTWARE\Microsoft\Windows NT\CurrentVersion", "UBR")`, which our invoker can technically drive (`is_static=true`) but `ParamKind::Other` bails on arrays so `EnumKey`/`GetStringValue` are out. | **E** for RTT/OS; **R** for UBR |
| 5.10 | **Host-vs-host Compare** | Three blockers. **(A)** the worker is single-host: `HOST`/`CRED`/`REMOTE` are `thread_local! RefCell`s on one thread (`worker.rs:443-451`) → A and B must be serialized with a `REMOTE` flush between. **(B)** no `Response` carries a host, so a `SetHost` issued mid-flight silently mislabels a result. **(C)** no stable row identity: `to_table` builds columns from the sorted union of `HashMap` keys, and **both** query paths strip system properties (local: `raw_query::<HashMap>` → `list_properties()` w/ `WBEM_FLAG_NONSYSTEM_ONLY`; alt-cred: `remote.rs:115` `BeginEnumeration(WBEM_FLAG_NONSYSTEM_ONLY.0)`), so `__RELPATH`/`__PATH`/`__CLASS` **never reach the UI**. Fix C: add `include_system: bool` to `Request::Query`; locally iterate `conn.exec_query(wql)` calling `get_property("__RELPATH")` by name; remotely pass `0` to `BeginEnumeration`. Then `diff_tables(a, b, key_cols, ignore_cols) -> TableDiff` — key columns default to the class's `is_key` properties, which `ClassSchema` already provides. Fix A/B properly: `HashMap<HostRef, WmiWorker>`, one COM thread per host. | **R** (C) + **E** (differ) + **medium refactor** (A/B) |
| 5.11 | **WmiPrvSE CPU / memory in the status bar and Providers** | `list_providers` selects only 4 columns; `process_names()` only `ProcessId, Name`. Widen to `SELECT ProcessId, Name, WorkingSetSize, PrivatePageCount, HandleCount, ThreadCount, KernelModeTime, UserModeTime FROM Win32_Process WHERE Name='WmiPrvSE.exe'` — but `KernelModeTime`/`UserModeTime` are **cumulative 100-ns ticks**, so %CPU needs two samples: `Δ(k+u)/(Δwall × logical_cpus)`. Instantaneous alternative: `SELECT IDProcess, PercentProcessorTime, WorkingSetPrivate, HandleCount, ThreadCount FROM Win32_PerfFormattedData_PerfProc_Process WHERE Name LIKE 'WmiPrvSE%'`, joined on `IDProcess` (**never on `Name`** — siblings are `WmiPrvSE`, `WmiPrvSE#1`, …). The quota ceiling to render against: `root` class `__ProviderHostQuotaConfiguration` (`MemoryPerHost`, `HandlesPerHost`, `ThreadsPerHost`) — this is the real "is a provider about to be killed for leaking" signal and the best thing in this whole view. | **E** (one widened query + a 2-sample delta) |
| 5.12 | **Namespace echo on responses** | `Instances`, `MethodDone`, `Mof`, `Providers`, `Network`, `EventSubscriptions` don't echo the namespace; a multi-namespace shell must track it out-of-band by id. Add the field. | **E** |
| 5.13 | **Query cancellation + row cap** | No cancellation exists. `SELECT * FROM CIM_DataFile` blocks the single worker thread indefinitely and `Shutdown` queues *behind* it, so **app exit hangs** (Drop joins). Needs `Request::Cancel{id}` + chunked `IEnumWbemClassObject::Next` with a finite `lTimeout` and an `AtomicBool` check per batch, plus `max_rows`. **Prerequisite for 5.6 and 5.7.** | **R** |
| 5.14 | **Alt-cred correctness (silent false negatives today)** | Only `q_maps`/`q_class_names` dispatch on `is_alt_cred()`. `list_connections` (`worker.rs:589`) calls `connect()` directly, so under alt-creds the TCP rows come from `RemoteConn` but the PID→name join goes over SSO as the *current* user → every `process` column silently blanks. Worse: `scan_subscriptions_in` guards the `__EventConsumer` walk with `if !is_alt_cred()` (`worker.rs:690`), so on a credentialed remote every subscription returns empty `consumer_type`/`action` and `assess("",q,"")` scores them **Low** — a silent false negative on the flagship security feature. `RemoteConn` needs an `exec_objects`/reflective path. `read_class_schema`, `class_mof`, `list_instances`, `invoke_method`, `build_search_index`, `list_classes_local` are likewise **SSO-only**. | **R** — and this is a *correctness bug*, not a design gap |
| 5.15 | **`Serialize` on responses** | Missing on `Response`, `ClassSchema`, `NetworkSnapshot`, `SearchIndex`, `MethodOutcome`, `SubscriptionDiff`, `ProviderDiff`. Needed for the Compare file format and any session save. | **E** |

### Design elements that are invented — the honest verdict

| Mock element | Verdict |
|---|---|
| **"Shared library · 24 queries · synced from `\\fileserv\wmi\library.json`"** (`dc.html:462`) | **DROP the "synced" claim.** There is no sync engine, no conflict resolution, no auth story, and building one is a product, not a feature. **Ship instead:** folders + favourites + `Import library…` / `Export library…` via `rfd`. A UNC path is a valid file path, so "import from a share" is honest and ~20 lines. Subtitle becomes `Local library · N queries · <path>`. |
| **`by` / author on saved-query cards** | **Real, free.** Record `std::env::var("USERNAME")` (+ `USERDOMAIN`) at save time. Do not fake. |
| **Certificate thumbprint** auth radio (`dc.html:571`) | **DROP.** DCOM/`IWbemServices` authentication via `CoSetProxyBlanket` + `COAUTHIDENTITY` supports user/password/domain under NTLM/Kerberos/Negotiate only. Certificate auth exists only for **WinRM/WSMan** (`WSManCreateSession` + a cert-thumbprint option), a transport this project does not implement. Shipping the radio would be a lie in a security tool. |
| **Transport segmented `WinRM \| DCOM`** + title-bar chip text "WinRM · Impersonate" | **The core speaks DCOM only.** Ship the segmented control with **WinRM disabled + a tooltip "WSMan transport not implemented"**, and make the chip read the *actual* transport (`DCOM · Impersonate`). Do not default-label it WinRM. |
| **Impersonation level dropdown** | **Real but partial.** `remote.rs` already hand-calls `CoSetProxyBlanket`, so `RPC_C_IMP_LEVEL_{IDENTIFY,IMPERSONATE,DELEGATE}` is one parameter away — **on the alt-cred path only**. On the SSO path we go through `wmi::WMIConnection`, which doesn't expose it. Ship it enabled only when alt-creds are on; otherwise show `Impersonate (default)` greyed. |
| **`RTT` column** | **Real** as DCOM connect+probe wall time. **Do not** present it as ICMP. `Win32_PingStatus.ResponseTime` exists but pings *from the connected host*, so under a remote connection it measures remote→target, not you→target — misleading. Label the column `RTT (bind)`. |
| **Machines "6 targets · 5 online"** green dots | **Real only if polled**, and each poll is a full DCOM bind. **Do not background-poll all targets.** Status is `Unknown` until the user hits Connect or Test; then it's cached with a timestamp. |
| **Status bar "WmiPrvSE 0.6% · 41 MB"** | **Real** (§5.11) but costs a WMI query every N s against the local box. Ship **opt-in** (Settings → "Show provider host stats", default off) rendering `WmiPrvSE — · — MB` when off. Never fake. |
| **Events "Sink queue" stat** | **Not observable** in a polling `IWbemObjectSink` design. **Replace** with the mpsc backlog we actually own (`MonitorMsg` depth) labelled "Queued", or drop the tile. |
| **Events Delivery = `Permanent`** | Implementable via `PutInstance` of `__EventFilter` + a consumer + `__FilterToConsumerBinding` — i.e. **writing the exact persistence artifact the Persistence view hunts**. Ship the segment **disabled in v1**. If ever enabled: explicit typed confirmation, `config::append_audit`, and a mandatory teardown button. |
| **Events "Delivery rate"** | **Real** — events/sec over a sliding window, computed client-side. |
| **"SELECT is projected server-side / ORDER BY is evaluated locally"** | **True statement about WQL**, worth keeping — but must be *derived* (case-insensitive scan for `ORDER BY` in the WQL), not hardcoded. |
| **Explorer CPU cell colored by threshold** | Real once the instance table knows a column is numeric+percent. Generic rule: color only when the column name matches a known percent metric; otherwise plain. Don't guess. |
| **`\\DC01-FRA` / `\\DC02-FRA` sample hosts, "1,142" class counts, "45 properties · 7 methods"** | Placeholder data in the mock. All become real once §5.1/5.2/5.7 land. **No placeholder strings ship** — empty states show `—` and a "Count classes" affordance. |

---

## 6. Risks, ranked

**R1 — Regression of the three shipping security features (highest).** Network, Persistence and Providers are the product's differentiator, are 1,150 lines of working code, and a full reskin touches every line. Two of them (`ui_persistence`, `ui_providers`) carry export + baseline + diff paths that have no tests.
*Mitigation:* they are **RESKIN, never rewrite** — the reskin commit for each must be diffable as "the same control flow with widget calls swapped". Before Phase 1, land a golden-file test per exporter (`subscriptions_to_csv/_json/_html`, `providers_to_json`, `query_to_csv/_json`, `events_to_json`) and round-trip tests for `subscriptions_from_json`/`providers_from_json` — those are pure functions with zero WMI dependency, so they're cheap and they pin the behavior the reskin must preserve. Reskin one view per commit, screenshot before/after.

**R2 — Custom chrome on Windows.** Snap Layouts are permanently lost; the 1px black top-edge artifact is guaranteed; transparency is unreliable on wgpu; the drag rect will eat button clicks if registered in the wrong order; resize strips will be swallowed by panels if registered too early.
*Mitigation:* the `decorated` escape hatch (§3.7) shipped and tested from day one, not retrofitted. A `shell/chrome.rs` doc comment enumerating the four ordering invariants. Manual QA matrix: 100/125/150% DPI × maximized/restored/snapped × decorated/undecorated.

**R3 — Dense-table performance and the sort/hover/rule combination.** Four tables today re-implement the same virtualized `TableBuilder` by hand; the new kit adds a per-row 8-vertex mesh rule, a per-cell painter for Compare, and per-row fade for Network. Plus: whole-row hover is one frame late, `Column::auto()` jitters under virtualization, and `Ui::response()` reads last-pass geometry so the first frame after a data change can misfire clicks.
*Mitigation:* `widgets/table.rs` is **one** implementation, virtualized (`body.rows`), with `Column::initial/exact/remainder + at_least + clip(true)` and **never** `Column::auto()`. Sort an `order: Vec<usize>` index, never the data. Benchmark with 50k synthetic rows in an example binary before wiring real views. Note `egui_extras`'s `serde` feature is **off** here (Cargo.lock confirms), so column widths use `get_temp` and are lost on restart — decide explicitly whether to enable it.

**R4 — Font licensing and binary size.** Four TTFs. Inter and JetBrains Mono are SIL OFL 1.1, Phosphor is MIT — all redistributable in a binary, **provided the licence texts ship**. Phosphor's full regular set is ~250 KB; Inter Regular+Medium ~600 KB; JetBrains Mono ~270 KB. That is ~1.1 MB added to a stripped release binary.
*Mitigation:* bundle `assets/fonts/LICENSE-*.txt`, add a `Settings → About → Licences` panel (also satisfies the OFL's "bundled with the software" clause), and add a third-party-notices section to README. If size matters, subset Phosphor to the ~120 used glyphs with `fonttools pyftsubset` (still MIT, still must ship the licence). **Do not** load fonts from Google Fonts at runtime as `styles.css:2` does — it would add a network dependency to a security tool.

**R5 — Scope.** Ten views, seven new-or-rebuilt, plus ~15 core capabilities, against a 2,973-line single-file GUI with no theme layer and no tests.
*Mitigation:* strict phasing where **every phase is releasable** (§7) and each phase's story is defensible on its own. Compare (view 5) is the single largest core-side risk (three blockers) and is deliberately last. Anything not in the task list below is out of scope for v1.0.

**R6 — The single-threaded worker becomes the bottleneck the new UI exposes.** The redesign asks for per-class counts, per-namespace counts, provider stats polling, live events, and A/B host queries — all through one COM thread with no cancellation, where a runaway query hangs even app exit.
*Mitigation:* `Request::Cancel` + chunked enumeration + `max_rows` (§5.13) is a **Phase 3 prerequisite**, not a nice-to-have. Nothing that triggers unbounded enumeration (counts) ships before it.

**R7 — Silent correctness regressions under alternate credentials.** §5.14 is a pre-existing bug the new Machines view will make far more visible (users will actually connect to remotes).
*Mitigation:* fix `list_connections`' dispatcher and the `scan_subscriptions_in` alt-cred guard **in Phase 5**, alongside Machines. Until fixed, the Machines view must show a persistent warning banner when alt-creds are active: "Schema, MOF, method invocation and consumer reflection use the current user's credentials."

**R8 — Icon-font fallback ordering.** If `icons` is inserted before the text font in a family, Phosphor steals shared codepoints and text renders as icons.
*Mitigation:* the `#[test]` in `theme/fonts.rs` asserting family vectors are `["ui","icons",…]` order, plus the codepoint-range test on `icons.rs`.

---

## 7. Phasing

Each phase is a releasable version with a coherent story.

| Phase | Version | Story | Gate |
|---|---|---|---|
| **P0** | `0.6.1` | *"Same app, refactored."* Module split, zero visual change, exporter golden tests. | Screenshot-identical to v0.6.0 |
| **P1** | `0.7.0` | *"VMI-Scope goes dark: the Nocturne design system."* Theme + fonts + icons + widget kit. All 5 existing tabs reskinned; old top bar/tab strip retained. | Every literal `Color32::from_rgb` gone from views |
| **P2** | `0.8.0` | *"A real application shell."* Custom title bar, 10-slot rail, status bar, command palette, Settings view, accent/density switching. | Palette can reach every view; `--decorated` works |
| **P3** | `0.9.0` | *"The Explorer, rebuilt."* 3-column + 5 sub-tabs, class kinds, qualifiers, derivation, associations, elapsed ms, instance counts. Requires cancellation first. | `CIM_DataFile` count is cancellable and app exit never hangs |
| **P4** | `0.10.0` | *"Query, Events and your library."* Query view w/ history, Events view w/ stream, Saved view w/ folders + import/export. | Saved query restores namespace *and* wql |
| **P5** | `0.11.0` | *"Targets."* Machines view, multi-host worker registry, host-stamped responses, RTT/OS, provider host stats, alt-cred correctness fixes. | Alt-cred subscription scan returns non-empty `consumer_type` |
| **P6** | `0.12.0` | *"Compare two machines."* System properties in query results, keyed table diff, Compare view. | A vs B on `Win32_Service` keys on `Name`, not whole-row equality |
| **P7** | `1.0.0` | *"Polish."* Settings completeness, licences panel, empty/error states, keyboard map, docs, accessibility pass. | No placeholder strings anywhere |

### The first commit

**`refactor(gui): extract free functions and constants out of app.rs`**

Move, with **zero behavior change**, the 11 module-scope free functions and 5 constants out of `app.rs` into `util.rs` (`smart_cmp`, `toggle_sort`, `is_dangerous_method`, `save_file`, `net_col_value`, `sub_col_value`, `prov_col_value`, `generate_script`) and `theme/tokens.rs` (`risk_color`, `state_color` rewritten against the token consts; `sortable_header` goes to `widgets/table.rs` as a placeholder). These are the only items in `app.rs` that take no `&self`, so the move is mechanical, reviewable in one screen, and cannot regress. It also creates `theme/` and `widgets/` as real modules on day one, so every subsequent commit has somewhere to land.

*Not* the full `app.rs` split — a 2,973-line single-struct file cannot be split in one reviewable commit. The struct split follows in tasks 0.4–0.9.

---

## 8. Task list

Tags: `[core]` `[gui]` `[theme]` `[infra]`. Every task is one commit.

### Phase 0 — Scaffolding (v0.6.1)

- [ ] **0.1** `[infra]` Add golden-file tests for `export::{query_to_csv, query_to_json}`. *AC: two fixtures committed; `cargo test -p vmiscope-core` passes.*
- [ ] **0.2** `[infra]` Golden tests for `export::{subscriptions_to_csv,_json,_html}` + round-trip `subscriptions_from_json`. *AC: HTML fixture byte-identical; round-trip is lossless.*
- [ ] **0.3** `[infra]` Golden tests for `export::{providers_to_json, events_to_json}` + round-trip `providers_from_json`, and unit tests for `diff_subscriptions`/`diff_providers` (added/removed/changed/empty). *AC: 8 tests, all green.*
- [ ] **0.4** `[gui]` Extract free fns + constants → `util.rs`, `theme/tokens.rs`, `widgets/table.rs`. **← FIRST COMMIT.** *AC: `cargo build` clean; app renders identically.*
- [ ] **0.5** `[gui]` Create `state/` module tree; move `PendingKind`, `alloc_id`, `push_error`, `error_log` → `state/{ids,errors}.rs`. *AC: `app.rs` no longer declares `PendingKind`.*
- [ ] **0.6** `[gui]` Move all 12 `request_*` methods → `state/requests.rs` as an `impl VmiScopeApp` block. *AC: `app.rs` shrinks by ~210 L; no signature changes.*
- [ ] **0.7** `[gui]` Move `handle_responses` → `state/responses.rs`. *AC: `app.rs` shrinks by ~180 L.*
- [ ] **0.8** `[gui]` Create `views/` tree; move `ui_network` → `views/network.rs`, `ui_persistence` + `load_baseline_dialog` → `views/persistence.rs`, `ui_providers` → `views/providers.rs`, `ui_events` → `views/events.rs`. *AC: `app.rs` < 1,300 L.*
- [ ] **0.9** `[gui]` Move `ui_namespace_tree`/`_node`, `ui_class_list`, `ui_central`, `ui_schema`, `ui_detail`, `ui_script_gen`, `ui_search` → `views/explorer/*`. Move the 4 windows → `overlays/*`. *AC: `app.rs` < 400 L, contains only struct + `new` + `ui`.*
- [ ] **0.10** `[gui]` Split `VmiScopeApp`'s 89 fields into per-domain sub-structs (`ExplorerState`, `QueryState`, `EventsState`, `SecurityState`, `ErrorState`) owned by `VmiScopeApp`. *AC: `VmiScopeApp` has < 15 direct fields.*
- [ ] **0.11** `[infra]` Add `#![deny(clippy::all)]`-clean pass + `cargo fmt`; add a CI-ish `check.ps1` running fmt/clippy/test. *AC: script exits 0.*
- [ ] **0.12** `[infra]` Tag v0.6.1, CHANGELOG entry "internal refactor, no user-visible change".

### Phase 1 — Nocturne theme + widget kit (v0.7.0)

- [ ] **1.1** `[infra]` Vendor `assets/fonts/{Inter-Regular,Inter-Medium,JetBrainsMono-Regular,Phosphor}.ttf` + `LICENSE-OFL.txt` + `LICENSE-MIT-phosphor.txt`. *AC: files present; README third-party-notices section added.*
- [ ] **1.2** `[theme]` `theme/tokens.rs`: all colors, 3 accent ramps, neutral ramp, status trio, radii, spacing consts. *AC: no `from_rgb` literal exists outside this file (grep gate).*
- [ ] **1.3** `[theme]` `theme/fonts.rs::install` — 4 fonts, 2 named families, icon fallback last in both built-ins. *AC: unit test asserts `Proportional == ["ui","icons",…]`.*
- [ ] **1.4** `[theme]` Generate `theme/icons.rs` from Phosphor's `style.css` (~120 consts). *AC: test asserts every const is one char in `U+E000..=U+F8FF`.*
- [ ] **1.5** `[theme]` `theme/mod.rs`: `Accent`, `Density`, `Theme`, `Metrics::for_density`. *AC: `Metrics` compiles for both densities.*
- [ ] **1.6** `[theme]` `theme::apply_accent(&mut Visuals, ramp)` fanning to all 6 sites. *AC: test asserts `selection.bg_fill`, `hyperlink_color`, `text_cursor.stroke`, `widgets.hovered.bg_stroke`, `widgets.active.{bg_stroke,bg_fill}` all derive from the ramp.*
- [ ] **1.7** `[theme]` `theme::install(ctx, theme)` — `all_styles_mut` + `text_styles` + spacing + scroll + `set_theme(Dark)`. *AC: app boots dark with Inter/JetBrains rendering; `global_theme_preference_switch` removed from the top bar.*
- [ ] **1.8** `[gui]` `widgets/rule.rs::{faded_hline, faded_vline, solid_hline}` via `epaint::Mesh`. *AC: an example screenshot shows a 1px rule fading over 48px at each end.*
- [ ] **1.9** `[gui]` `widgets/button.rs`: `btn_primary` (accent outline, never filled), `btn_secondary` (divider outline), `btn_ghost`, `btn_icon`, hover/active tints from the ramp. *AC: no `Button::fill` remains anywhere.*
- [ ] **1.10** `[gui]` `widgets/button.rs::focus_ring(ui,&resp)` — 2px accent, offset 2, `StrokeKind::Outside`. *AC: tabbing through the kit shows the ring on every control.*
- [ ] **1.11** `[gui]` `widgets/button.rs::segmented(ui, &mut T, &[(T,&str)])`. *AC: renders a 1-of-N control matching the mock's `.seg`.*
- [ ] **1.12** `[gui]` `widgets/chip.rs`: `tag`, `tag_accent`, `count_pill`, `kind_badge` (15px square C/A/E), `dot_chip`. *AC: all five render at the mock's sizes.*
- [ ] **1.13** `[gui]` `widgets/field.rs`: `mono_input`, `filter_box` (leading Phosphor magnifier, mono, hint), `labelled_row`, `radio_group`, `combo`. *AC: `filter_box` replaces the 4 hand-rolled filters + 3 literal "🔍" labels.*
- [ ] **1.14** `[gui]` `widgets/card.rs`: surface card = `Frame` w/ `R_MD` + 1px hairline + `Shadow{offset:[0,6],blur:18}`; `card_grid(min_w)` helper. *AC: cards never stack heavy shadows.*
- [ ] **1.15** `[gui]` `widgets/kv.rs::kv_grid` replacing the 4 hand-rolled detail Grids (`detail-grid`, `act-out`, `confirm-grid`, `schema-props`). *AC: all four call sites use it.*
- [ ] **1.16** `[gui]` `widgets/loading.rs`: themed spinner + skeleton row + `elapsed_badge(ms)`. *AC: all 13 inline `ui.spinner()` sites replaced.*
- [ ] **1.17** `[gui]` `widgets/table.rs` core: `DataTable` builder — id_salt, columns (`initial/exact/remainder` + `at_least` + `clip`), `sense(click)`, `cell_layout`, virtualized `body.rows`, index-vector sorting. *AC: renders the Providers table identically to today.*
- [ ] **1.18** `[gui]` `widgets/table.rs`: sortable header — 11px uppercase, `extra_letter_spacing(0.5)`, ▲/▼, tri-state asc→desc→off, click collected into a local and applied after the closure. *AC: header click cycles all three states.* **Note:** the header cell response only fires because the builder sets `.sense(Sense::click())`; without it, `resp.clicked()` is always false.
- [ ] **1.19** `[gui]` `widgets/table.rs`: faded 48px row rule painted per visible row (8% text tint) + hover 4% via `body.ui_mut().visuals_mut().widgets.hovered.bg_fill`. *AC: rules visibly fade at both ends; hover tints without hiding the rule.*
- [ ] **1.20** `[gui]` `widgets/table.rs`: selection (`set_selected` before the first `col`) with `selection.stroke.color` set so selected-row text is legible. *AC: a selected row's text is not silently recolored to something unreadable.*
- [ ] **1.21** `[gui]` `widgets/table.rs`: right-aligned numeric cells via `ui.with_layout(right_to_left(Center))` (**not** `Label::halign`, which doesn't work in a cell), plus `numeric_threshold_color` helper. *AC: PID/WS/CPU columns right-align.*
- [ ] **1.22** `[gui]` `widgets/table.rs`: ellipsized path cells via `Column::clip(true)` + built-in `show_tooltip_when_elided`. *AC: a long path shows '…' and a tooltip only when truncated.*
- [ ] **1.23** `[gui]` `widgets/table.rs`: per-row alpha (`gamma_multiply`) for Network's fade-on-close, and per-cell background painter for Compare. Document the `clip_rect_margin == 0.0` trap. *AC: a doc comment states painting outside `max_rect` in a clipped column is discarded.*
- [ ] **1.24** `[gui]` `widgets/codeview.rs`: line-numbered gutter + mono panel + copy button. *AC: renders WQL with line numbers.*
- [ ] **1.25** `[gui]` `widgets/codeview.rs`: token tinting for WQL / PowerShell / C# / VBScript / MOF (keyword=accent-300, string=ok, comment=neutral-600, number=warn). *AC: MOF panel is visibly syntax-colored.*
- [ ] **1.26** `[gui]` `widgets/export_menu.rs`: `Popup::menu` dropdown — CSV / JSON typed / Tab-separated / Copy as table, with right-aligned shortcut hints. *AC: replaces the inline export clusters in Query, Persistence, Providers, Events.*
- [ ] **1.27** `[gui]` RESKIN `views/network.rs` onto the kit. *AC: same columns, same fade, same filters; zero color literals.*
- [ ] **1.28** `[gui]` RESKIN `views/persistence.rs`. *AC: risk pills use OK/WARN/BAD; exports + baseline + diff unchanged; golden tests still pass.*
- [ ] **1.29** `[gui]` RESKIN `views/providers.rs`. *AC: snapshot/diff unchanged.*
- [ ] **1.30** `[gui]` RESKIN `views/events.rs` (kit only; the full redesign is P4). *AC: monitor start/stop unchanged.*
- [ ] **1.31** `[gui]` RESKIN `views/explorer/*` and `overlays/*` onto the kit, keeping today's layout. *AC: all remaining unicode-escape icons replaced by Phosphor consts.*
- [ ] **1.32** `[theme]` Replace `risk_color`/`state_color` bodies with token lookups; delete all 17 `from_rgb` literal sites. *AC: `grep -c "from_rgb" src/views src/overlays` == 0.*
- [ ] **1.33** `[infra]` `check.ps1` gains a grep gate: no `from_rgb`/`from_gray` outside `theme/tokens.rs`, no `ui.separator()`, no `RichText::strong()`. *AC: gate fails on a deliberate violation.*
- [ ] **1.34** `[infra]` Release v0.7.0 + CHANGELOG + README screenshots.

### Phase 2 — Shell (v0.8.0)

- [ ] **2.1** `[infra]` Add `winit = "0.30.13"` to `vmiscope-gui`; add `egui_extras = { features = ["image"] }` **only if** an in-bar bitmap is used (otherwise skip — the glyph is a font icon). *AC: `cargo tree` shows one winit version.*
- [ ] **2.2** `[gui]` `main.rs`: `with_decorations(cfg.decorated)`, min size → `[980, 560]`, `.with_icon`. *AC: undecorated window opens.*
- [ ] **2.3** `[gui]` `VmiScopeApp::new`: `cc.winit_window()` → `set_corner_preference(Round)` behind `#[cfg(windows)]` + `!decorated`. *AC: corners are rounded on Win11.*
- [ ] **2.4** `[gui]` `shell/chrome.rs`: outer shell `Frame` (BG + 1px divider stroke + `R_LG`) on a `CentralPanel`. *AC: a hairline border traces the window.*
- [ ] **2.5** `[gui]` `shell/chrome.rs::title_drag(ui) -> Response` registered **before** the title panel; `double_clicked` → `Maximized(!is_max)` else `drag_started_by(Primary)` → `StartDrag`. *AC: dragging moves the window; double-click toggles maximize; title-bar buttons still click.*
- [ ] **2.6** `[gui]` `shell/chrome.rs::resize_strips(ui)` — 8 strips, `BeginResize` + cursor icons, registered **after** all panels. *AC: all 8 directions resize; cursors change.*
- [ ] **2.7** `[gui]` `shell/titlebar.rs`: 40px `Panel::top` with `exact_size` + `resizable(false)` + `show_separator_line(false)` + `Frame::NONE`; own 1px divider hline. *AC: measured content height is 40px, no double separator.*
- [ ] **2.8** `[gui]` Title bar left cluster: 22px accent-outlined glyph + "WMI Explorer" + version pill from `env!("CARGO_PKG_VERSION")`. *AC: pill reads `v0.8.0`.*
- [ ] **2.9** `[gui]` Machine chip: status dot + `\\HOST` mono + divider + transport/impersonation text + caret; click → `View::Machines`. *AC: text reads `DCOM · Impersonate` locally, not `WinRM`.*
- [ ] **2.10** `[gui]` Palette trigger box: magnifier + placeholder + `Ctrl K` pill; click opens the palette. *AC: click and Ctrl-K reach the same state.*
- [ ] **2.11** `[gui]` Live/Paused ghost toggle (`ph-pulse`) bound to `net_paused` + monitor state; Refresh ghost button bound to the active view's refresh. *AC: pausing Network stops the 1.5s poll.*
- [ ] **2.12** `[gui]` Three 38×40 window buttons; close hovers to `BAD.gamma_multiply(0.30)`; maximize icon swaps on `viewport().maximized`. *AC: all three work; icon reflects state.*
- [ ] **2.13** `[gui]` `shell/rail.rs`: 64px `Panel::left` with **explicit** `.resizable(false)`; 10 destinations in 3 groups + bottom cluster; 17px icon over 9px label; selected = accent fg + 15% accent bg pill. *AC: no hover separator flash on the rail edge.*
- [ ] **2.14** `[gui]` `views/mod.rs`: `View` enum (10) replacing `Tab` (5); dispatch + per-view icon/label/context-string. *AC: every rail item reaches a view; the old tab strip is deleted.*
- [ ] **2.15** `[gui]` `shell/statusbar.rs`: 24px bar, live dot + connection + context + right cluster + error-log toggle. *AC: `Log (n)` still opens the error log.*
- [ ] **2.16** `[gui]` `overlays/palette.rs`: `egui::Modal` re-anchored via `Modal::default_area(id).anchor(CENTER_TOP, vec2(0,120))`, `ui.set_width(560.0)`. *AC: opens centered-top at a fixed width.*
- [ ] **2.17** `[gui]` Palette input autofocus + select-all: `TextEdit::show` → `out.response.request_focus()` + `state.cursor.set_char_range` + **`state.store`**. *AC: text is selected on open.*
- [ ] **2.18** `[gui]` Palette arrow-key nav: consume `ArrowUp`/`ArrowDown`/`Enter`/`Escape` via `ctx.input_mut(..).consume_key(..)` **before** the `TextEdit` is added. *AC: arrows move the selection without moving the caret.*
- [ ] **2.19** `[gui]` Palette grouped results (Class / Property / Method / Command) with per-group icons; first row pre-highlighted; `scroll_to_me`. *AC: reuses `compute_hits` + `apply_search_hit` unchanged.*
- [ ] **2.20** `[gui]` Palette Command group: every rail destination + Refresh + Run query + Export + Toggle live + accent/density switches. *AC: ≥ 18 commands reachable.*
- [ ] **2.21** `[gui]` Global shortcuts: `Ctrl+K` palette, `F5` refresh, `Ctrl+Enter` run query, `Esc` close overlay — via `KeyboardShortcut` + `consume_shortcut`, most-specific first. *AC: none fire while a `TextEdit` has focus except Ctrl-K/F5.*
- [ ] **2.22** `[gui]` `views/settings.rs` skeleton: 4 groups with accent-underlined `h6` headings + labelled setting rows. *AC: rows render with key, note, mono value, action icon.*
- [ ] **2.23** `[gui]` Settings → Interface: accent (steel/teal/amber) + density (compact/comfortable) + monospace font; each writes `Config` and calls `theme::install`. *AC: accent switches in one frame and survives restart.*
- [ ] **2.24** `[gui]` `Config` v2: `accent`, `density`, `decorated`, `show_provider_stats`, `default_namespace`, `row_limit`, `impersonation`, `default_lang`, `line_width`, `show_system_classes`, `byte_format`, `live_polling`; versioned with a migration from v1. *AC: an old `config.json` loads without loss.*
- [ ] **2.25** `[gui]` `--decorated` CLI flag + Settings toggle (requires restart); title bar hides window buttons and chrome skips drag/resize when set. *AC: decorated mode has an OS title bar and no double chrome.*
- [ ] **2.26** `[gui]` Delete `ui_connection_bar` from the top bar (its logic moves to `state/machines.rs` for P5; a temporary minimal connect popover keeps remote connect reachable). *AC: remote connect still works.*
- [ ] **2.27** `[infra]` DPI/state QA matrix (100/125/150% × maximized/restored/snapped × decorated/undecorated); record results in the PR. *AC: 12 cells, no layout break.*
- [ ] **2.28** `[infra]` Release v0.8.0.

### Phase 3 — Explorer rebuild (v0.9.0)

- [ ] **3.1** `[core]` Add `Request::Cancel { id }`; worker holds `HashMap<u64, Arc<AtomicBool>>`; long enumerations check it per batch. *AC: a cancelled query stops within one batch.*
- [ ] **3.2** `[core]` Convert `run_query` to chunked `IEnumWbemClassObject::Next(timeout, &mut [None;64], &mut n)` with a finite `lTimeout`, plus `max_rows: Option<usize>` on `Request::Query`. *AC: `SELECT * FROM CIM_DataFile` respects a 5,000-row cap and cancels.*
- [ ] **3.3** `[core]` Handle `Shutdown` out-of-band (a second priority flag checked per batch) so app exit never queues behind a long query. *AC: closing the window during a `CIM_DataFile` query exits in < 1s.*
- [ ] **3.4** `[core]` Add `elapsed_ms: u64` to `QueryResult`; instrument `run_query` with `Instant`, split connect vs execute. *AC: `Response::QueryResult` carries a plausible non-zero ms.*
- [ ] **3.5** `[core]` Add `elapsed_ms` to `Classes`, `ChildNamespaces`, `Schema`, `SearchIndex`, `Providers`, `Network`, `EventSubscriptions`. *AC: all seven carry timing.*
- [x] **3.6** `[core]` `reflect.rs`: stop filtering class qualifiers; add `qualifiers: Vec<(String,String)>` to `ClassSchema`. *AC: ~~`Dynamic`, `Provider`, `UUID`, `Association`, `Singleton`, `Supports*` all appear for `Win32_Process`~~ — **corrected against live WMI**: `Win32_Process` carries neither `Association` nor `Singleton`, so the AC as written was unachievable. Real AC: `dynamic`, `provider`, `UUID`, `CreateBy`, `DeleteBy`, `Locale`, `SupportsCreate`, `SupportsDelete`. Note the casing — `root\CIMV2` returns `dynamic` and `provider` lowercase but `Association`, `Singleton` and `UUID` capitalized, and `Abstract` both ways depending on the class, so every qualifier comparison must be case-insensitive.*
- [ ] **3.7** `[core]` `reflect.rs`: read `__Derivation` **by name** (never via `list_properties`); add `derivation: Vec<String>` to `ClassSchema`. *AC: `Win32_Process` → `["CIM_Process","CIM_LogicalElement","CIM_ManagedSystemElement"]`.*
- [ ] **3.8** `[core]` `ClassKind` bitflags (Dynamic/Static/Association/Event/System/Abstract/Singleton/Perf) derived from qualifiers + derivation; field on `ClassSchema`. *AC: unit-tested classification for 6 known classes.*
- [ ] **3.9** `[core]` `Response::Classes` gains `Vec<ClassBrief{name, kind, provider}>`; `list_classes_local` fills `kind` lazily (name-prefix + a cached schema hit) without an extra round trip per class. *AC: enumerating `root\CIMV2` is no slower than today.*
- [ ] **3.10** `[core]` `Request::NamespaceStats{id, namespace, recursive}` using `CreateClassEnum(DEEP|FORWARD_ONLY|RETURN_IMMEDIATELY)` counted without reading `__CLASS`. *AC: `root\CIMV2` returns a count in < 400 ms.*
- [ ] **3.11** `[core]` `Request::InstanceCount{id, namespace, class, deep}` via `CreateInstanceEnum(SHALLOW|FORWARD_ONLY|RETURN_IMMEDIATELY)` + batched `Next`, cancellable, with a per-class deadline. *AC: `CIM_DataFile` aborts on the deadline and reports `Partial(n)`.*
- [ ] **3.12** `[core]` Skip-list for counting: abstract, association and `__Event`-derived classes are never counted. *AC: their badges show `—`, not `0`.*
- [ ] **3.13** `[core]` `Request::Associations{id, namespace, class}` → `REFERENCES OF {C} WHERE SchemaOnly` + `ASSOCIATORS OF {C} WHERE SchemaOnly` via `exec_query(&str)`; new `AssocInfo{assoc_class, role, target_class, note}`. *AC: `Win32_Process` returns ≥ 4 associations.*
- [ ] **3.14** `[core]` `reflect::read_params`: record `IN`/`OUT` qualifiers so a param in both signatures is marked, not duplicated. *AC: `Win32_Process.Create` shows no duplicate `ProcessId`.*
- [x] **3.15** `[core]` Static-method robustness: treat "no `Key` property" and `Singleton` as static-capable; retry the class-path invocation on ~~`WBEM_E_ILLEGAL_OPERATION`~~ **`WBEM_E_INVALID_METHOD` (0x8004102E)** — measured: that is what WMI actually returns for `Win32_Process.Create` aimed at an instance path. Both codes are handled, and a test asserts access-denied and not-found are *not* swallowed by the retry. *AC: a static method lacking the `Static` qualifier invokes without demanding an instance — `Win32_OperatingSystem` carries the qualifier on none of its five methods, and keyless non-abstract classes (`CIM_USBDevice`, `CIM_USBHub`, `CIM_StorageVolume`) are class-path-only because WMI cannot address an instance of them at all.*
- [ ] **3.16** `[gui]` `views/explorer/mod.rs`: 3-column layout — `Panel::left("vs_ns_tree").exact_size(224)`, `Panel::left("vs_class_list").exact_size(290)`, central detail. *AC: columns land at exactly 224/290.*
- [ ] **3.17** `[gui]` `views/explorer/tree.rs`: 13px/level indent, caret + folder icons, per-namespace class count, footer "N namespaces · M ms". *AC: counts populate lazily from `NamespaceStats`.*
- [ ] **3.18** `[gui]` `views/explorer/classlist.rs`: mono filter box + chips All/Dynamic/Association/Event/System, driven by `ClassKind`. *AC: each chip filters correctly and shows a live count.*
- [ ] **3.19** `[gui]` `views/explorer/classlist.rs`: 15px C/A/E kind badge + instance count + footer "N of M classes · chip". *AC: counts appear only after an explicit "Count" action or on selection.*
- [ ] **3.20** `[gui]` `views/explorer/detail.rs`: breadcrumb `\\host > ns > class` + copy icon (`ctx.copy_text`). *AC: copy puts the full object path on the clipboard.*
- [ ] **3.21** `[gui]` `views/explorer/detail.rs`: H4 class name + tags (Dynamic, provider) + meta line "N properties · M methods · derives from X". *AC: derivation shows the immediate parent from `ClassSchema.derivation[0]`.*
- [ ] **3.22** `[gui]` `views/explorer/detail.rs`: action row Query / Watch / Invoke / Export dropdown. *AC: Query hands the class off to `View::Query` with a prefilled WQL.*
- [ ] **3.23** `[gui]` Sub-tab strip with counts: Instances | Properties | Methods | Schema | Code. *AC: counts come from real data; the tab persists per class.*
- [ ] **3.24** `[gui]` `views/explorer/instances.rs`: dense sortable table, mono cells, right-aligned numerics, threshold-colored percent columns, ellipsized path. *AC: 5,000 rows scroll at 60 fps.*
- [ ] **3.25** `[gui]` `views/explorer/properties.rs`: instance-path header + table Property | CIM type | Value | Qualifiers; per-property icon (key/pencil/hash/text/clock); CIM type in accent-300. *AC: `Win32_Process.Handle` shows the key glyph in accent.*
- [ ] **3.26** `[gui]` `views/explorer/methods.rs`: card grid `minmax(330px)`, function icon + name + scope tag + Invoke button + mono signature + note. *AC: static/instance tag matches `is_static`.*
- [ ] **3.27** `[gui]` `views/explorer/schema.rs` left column: Derivation chain (indented, current class in accent) + Associations (link icon + name + note). *AC: both populate from 3.7/3.13.*
- [ ] **3.28** `[gui]` `views/explorer/schema.rs` right column: class-qualifiers table + MOF panel (mono, surface bg, syntax-colored). *AC: MOF loads inline; the floating MOF window is deleted.*
- [ ] **3.29** `[gui]` `views/explorer/code.rs`: segmented PowerShell | C# | VBScript | WQL + Copy + "Save as script"; line-numbered panel. *AC: all four languages generate.*
- [ ] **3.30** `[core]` `generate_script`: add C# (`System.Management`) and WQL arms; `ScriptLang` gains 2 variants. *AC: the C# output compiles as written.*
- [ ] **3.31** `[gui]` `overlays/invoke.rs`: `egui::Modal` merging today's actions panel + confirm window — signature line, per-param fields with a "required" marker, live command preview, result panel (ReturnValue + out params + elapsed), "Runs under DOMAIN\user", Cancel/Execute. *AC: the dangerous-method gate still fires and still writes `append_audit`.*
- [ ] **3.32** `[gui]` Delete the right-hand actions panel and the confirm `Window`. *AC: `actions_open`/`confirm_open` fields removed.*
- [ ] **3.33** `[gui]` Explorer empty/loading/error states (no namespace, no class, count unavailable, count partial). *AC: no view ever shows a blank rectangle.*
- [ ] **3.34** `[infra]` Release v0.9.0.

### Phase 4 — Query, Events, Saved (v0.10.0)

- [ ] **4.1** `[gui]` `views/query.rs`: WQL editor with a line-number gutter (`widgets::codeview`) + Run/Save/Export. *AC: gutter tracks wrapped lines correctly.*
- [ ] **4.2** `[gui]` Query status strip: "Completed in N ms · M rows · SELECT is projected server-side" from real `elapsed_ms`/`rows.len()`. *AC: the ms is the measured value, never a constant.*
- [ ] **4.3** `[gui]` Query warn note "ORDER BY is evaluated locally by the client" shown **only** when a case-insensitive `ORDER BY` appears outside a string literal. *AC: a query without ORDER BY shows no note.*
- [ ] **4.4** `[gui]` Query result table on the kit + row-detail via the palette-style side reveal (replacing today's right `detail` panel). *AC: clicking a row shows its full property set.*
- [ ] **4.5** `[gui]` `Panel::right("vs_history").exact_size(262)`: query text, elapsed ms (colored warn when > 1000), row count, relative time, click-to-reload. *AC: clicking a history item reloads text + namespace.*
- [ ] **4.6** `[gui]` `Config.history` entries become `HistoryEntry{wql, namespace, elapsed_ms, rows, at}`; migrate from `Vec<String>`. *AC: an old config's plain strings load with `None` metadata.*
- [ ] **4.7** `[gui]` Debounce `Config::save()` — today `push_history` writes `config.json` to disk on **every** query run. *AC: 10 rapid queries produce ≤ 2 writes.*
- [ ] **4.8** `[gui]` `views/events.rs` left column (300px): event-query textarea, `WITHIN` interval field, Delivery segmented (Permanent **disabled** + tooltip), Start/Stop + Clear. *AC: the WITHIN value is injected into the WQL.*
- [ ] **4.9** `[gui]` Events stats card: Events received, Delivery rate (events/s over a 30s window), Provider, **Queued** (real mpsc backlog — "Sink queue" is not observable). *AC: rate is computed, never constant.*
- [ ] **4.10** `[gui]` Events stream header: pulsing live dot + "N events since HH:MM:SS" + Filter + Save log. *AC: the pulse is driven by `input().time`, not `animate_bool` (which cannot loop).*
- [ ] **4.11** `[gui]` Event row: timestamp | kind badge (Creation=ok / Modification=accent / Deletion=bad / Operation=muted) | class | detail | open icon. *AC: kind is parsed from `__CLASS` of the event.*
- [ ] **4.12** `[gui]` New-row accent flash-in: per-row `created_at` from `input().time` + `easing::cubic_out((age/0.18).clamp(0,1))`. **Not** `animate_bool`, which returns the target on the first frame for a fresh Id and would never fade in. *AC: a new row visibly flashes accent once.*
- [ ] **4.13** `[gui]` Events log capped ring buffer (replacing the 500-element `Vec` front-insert + truncate). *AC: sustained 200 ev/s does not degrade frame time.*
- [ ] **4.14** `[gui]` `views/saved.rs`: card grid — star/favourite, title, folder tag, mono query preview, meta (ms · rows · author), New query. *AC: cards render at `minmax(310px)`.*
- [ ] **4.15** `[gui]` `SavedQuery` v2: `+folder, +fav, +author, +last_ms, +last_rows`; author = `USERNAME`/`USERDOMAIN` at save time. *AC: old saved queries migrate with empty folder and `fav=false`.*
- [ ] **4.16** `[gui]` **Fix:** applying a saved query restores `namespace` **and** `wql` (today `app.rs:1142-1145` restores only `wql`, silently running against the wrong namespace). *AC: a query saved under `root\subscription` reopens there.*
- [ ] **4.17** `[gui]` Saved library Import…/Export… via `rfd` (a UNC path is a valid file path). Header reads `Local library · N queries · <path>` — **no "synced" claim**. *AC: export→import round-trips losslessly.*
- [ ] **4.18** `[gui]` Folder filter + favourites filter on the Saved view. *AC: both filter live.*
- [ ] **4.19** `[infra]` Release v0.10.0.

### Phase 5 — Machines + multi-host (v0.11.0)

- [ ] **5.1** `[core]` Add `host: Option<String>` to **every** `Response` variant, stamped at *execution* time. *AC: a `SetHost` mid-flight can no longer mislabel a result.*
- [ ] **5.2** `[core]` Add `namespace` to `Instances`, `MethodDone`, `Mof`, `Providers`, `Network`, `EventSubscriptions`. *AC: no response requires out-of-band namespace tracking.*
- [ ] **5.3** `[core]` `HostRef` + `WorkerRegistry: HashMap<HostRef, WmiWorker>` — one COM thread per host; `Request` gains a `target: HostRef`. *AC: two hosts can be queried without a `SetHost` flush between them.*
- [ ] **5.4** `[core]` `Response::HostConnected{id, host, connect_ms, probe_ms, os: HostInfo}`; the probe result (`worker.rs:422`) is no longer discarded. *AC: connect returns a real bind time.*
- [ ] **5.5** `[core]` `HostInfo` from `SELECT Caption, Version, BuildNumber, OSArchitecture, LastBootUpTime FROM Win32_OperatingSystem` + `SELECT UUID FROM Win32_ComputerSystemProduct`. *AC: build number and machine UUID populate.*
- [ ] **5.6** `[core]` **Bug fix:** `list_connections` (`worker.rs:589`) must route through the credential dispatcher instead of calling `connect()` directly. *AC: under alt-creds the `process` column is populated, not blank.*
- [ ] **5.7** `[core]` `RemoteConn::exec_objects` returning `IWbemClassObject`s (its `object_to_map` already has them and discards `__CLASS`). *AC: a reflective walk is possible over alt-creds.*
- [ ] **5.8** `[core]` **Bug fix:** remove the `if !is_alt_cred()` guard on the `__EventConsumer` walk (`worker.rs:690`) using 5.7. *AC: a credentialed remote subscription scan returns non-empty `consumer_type`/`action` and scores above Low where warranted.*
- [ ] **5.9** `[core]` Route `read_class_schema`, `class_mof`, `list_instances`, `invoke_method`, `build_search_index`, `list_classes_local` through the credential dispatcher. *AC: schema loads over alt-creds.*
- [ ] **5.10** `[core]` `CoSetProxyBlanket` impersonation level parameterised (`IDENTIFY`/`IMPERSONATE`/`DELEGATE`) on the alt-cred path. *AC: the setting changes the blanket call.*
- [ ] **5.11** `[core]` `Msft_Providers` query widened with `HostingModel`, `HostingSpecification`, `user`, `Result`. *AC: new columns available.*
- [ ] **5.12** `[core]` Provider host stats: `SELECT IDProcess, PercentProcessorTime, WorkingSetPrivate, HandleCount, ThreadCount FROM Win32_PerfFormattedData_PerfProc_Process WHERE Name LIKE 'WmiPrvSE%'`, joined on `IDProcess` (**never** on `Name`). *AC: sibling hosts `WmiPrvSE#1/#2` are distinguished.*
- [ ] **5.13** `[core]` `__ProviderHostQuotaConfiguration` (namespace `root`) → `MemoryPerHost`, `HandlesPerHost`, `ThreadsPerHost`. *AC: quota ceilings available for the usage bars.*
- [ ] **5.14** `[gui]` Status-bar provider stats, **opt-in** via `Config.show_provider_stats` (default off), rendering `WmiPrvSE — · — MB` when off. *AC: no WMI query runs when disabled.*
- [ ] **5.15** `[gui]` Providers view gains CPU / private bytes / handles / threads columns + a usage bar against the quota. *AC: a leaking provider is visually obvious.*
- [ ] **5.16** `[gui]` `views/machines.rs` targets table: Target, Transport, Credential, RTT (bind), OS build, Status w/ colored dot. *AC: Status is `Unknown` until Connect/Test — no background polling.*
- [ ] **5.17** `[gui]` `Panel::right("vs_new_conn").exact_size(290)` New-connection panel: computer name, namespace, Transport segmented (**WinRM disabled + tooltip**), Authentication radios (Current user Kerberos / Alternate credentials — **Certificate thumbprint dropped**), Impersonation level, Connect + Test. *AC: the panel contains no unimplementable control.*
- [ ] **5.18** `[gui]` `Config.targets: Vec<Target{name, namespace, transport, cred_ref, last_rtt_ms, last_os, last_seen}>`; passwords are **never** persisted. *AC: `config.json` contains no password field.*
- [ ] **5.19** `[gui]` Machines alt-cred warning banner listing which operations still run as the current user (until 5.9 lands fully). *AC: banner disappears once 5.9 is complete.*
- [ ] **5.20** `[gui]` Title-bar machine chip reads live transport + impersonation from the active target. *AC: switching targets updates the chip.*
- [ ] **5.21** `[infra]` Release v0.11.0.

### Phase 6 — Compare (v0.12.0)

- [ ] **6.1** `[core]` `Request::Query` gains `include_system: bool`. Local path stops using `raw_query::<HashMap>` and calls `get_property("__RELPATH")`/`"__PATH"`/`"__CLASS"` by name; remote path passes `0` instead of `WBEM_FLAG_NONSYSTEM_ONLY.0` at `remote.rs:115`. *AC: `__RELPATH` appears as a column when requested.*
- [ ] **6.2** `[core]` `QueryResult` gains `key_columns: Vec<String>` populated from the class's `is_key` properties when the WQL targets a single class. *AC: `Win32_Service` returns `["Name"]`.*
- [ ] **6.3** `[core]` `diff::diff_tables(a, b, key_cols, ignore_cols) -> TableDiff{added, removed, changed, unchanged}`. *AC: unit-tested on 4 synthetic pairs including a volatile-column ignore.*
- [ ] **6.4** `[core]` `#[derive(Serialize)]` on `Response`, `ClassSchema`, `NetworkSnapshot`, `SearchIndex`, `MethodOutcome`, `SubscriptionDiff`, `ProviderDiff`, `TableDiff`. *AC: a full compare result serializes to JSON.*
- [ ] **6.5** `[gui]` `views/compare.rs`: A/B host pickers (from `Config.targets`) + class/WQL picker + Run. *AC: A and B run against different `HostRef`s without a `SetHost` race.*
- [ ] **6.6** `[gui]` Diff legend (Identical / Changed / Only on A / Only on B) + Export diff (JSON + CSV). *AC: export round-trips.*
- [ ] **6.7** `[gui]` Diff table: sign column (`=` `≠` `−` `+`) + per-cell tinted backgrounds. Value columns are **unclipped** so the tint is not discarded (`clip_rect_margin == 0.0`). *AC: tints are visible on every changed cell.*
- [ ] **6.8** `[gui]` Auto-key from `key_columns` with a manual override combo; ignore-columns multiselect defaulting to volatile columns (PID, timestamps, counters). *AC: `Win32_Service` diffs on `Name`, not whole-row equality.*
- [ ] **6.9** `[gui]` Compare empty/partial states (one side failed, one side unauthorized, differing schemas). *AC: a failed B side shows an error card, not an empty table.*
- [ ] **6.10** `[infra]` Release v0.12.0.

### Phase 7 — Polish + 1.0 (v1.0.0)

- [ ] **7.1** `[gui]` Settings → Connection: default namespace, impersonation level, authentication, operation timeout — all wired to real behavior. *AC: no setting is decorative.*
- [ ] **7.2** `[gui]` Settings → Results: row limit (→ `Request::Query.max_rows`), live polling toggle, byte formatting, show system classes. *AC: row limit visibly truncates.*
- [ ] **7.3** `[gui]` Settings → Code generation: default language, include-credentials block, line width. *AC: generated scripts respect all three.*
- [ ] **7.4** `[gui]` Settings → About/Licences panel rendering the bundled OFL + MIT texts. *AC: satisfies the OFL bundling clause.*
- [ ] **7.5** `[gui]` Keyboard map overlay (`?` or `F1`) listing every shortcut. *AC: matches the actual `KeyboardShortcut` registrations.*
- [ ] **7.6** `[gui]` Empty/loading/error state audit across all 10 views. *AC: a checklist of 30 states, all covered.*
- [ ] **7.7** `[gui]` Focus-ring audit: every interactive widget in the kit and every view calls `focus_ring`. *AC: full keyboard traversal shows a ring at every stop.*
- [ ] **7.8** `[gui]` Move the remaining blocking UI-thread work off the frame: `rfd` dialogs and `Config::save` (today `save_file`, `load_baseline_dialog`, the providers baseline path, and `push_history` all block the UI thread). *AC: opening a file dialog does not stall the frame loop.*
- [ ] **7.9** `[gui]` `ViewportCommand::Close` gate: if a method invocation is in flight, answer with `CancelClose` and confirm. *AC: closing mid-invoke prompts.*
- [ ] **7.10** `[infra]` Perf benchmark example: 50k-row table, 1,400-class list, 2k-event stream. *AC: all three hold 60 fps.*
- [ ] **7.11** `[infra]` Decide and document `egui_extras` `serde` feature (column widths currently use `get_temp` and are lost on restart). *AC: decision recorded in CHANGELOG.*
- [ ] **7.12** `[infra]` README rewrite + new screenshots + CHANGELOG for 1.0. *AC: every screenshot is the Nocturne shell.*
- [ ] **7.13** `[infra]` `cargo deny check` against the new deps (winit, fonts are data not deps). *AC: `deny.toml` passes.*
- [ ] **7.14** `[infra]` Tag v1.0.0.

### Phase 4B — Process view (v0.10.1)

Rationale, data model and the privilege caveat are in §9. Ordered after Phase 4
because it reuses the events kit (kind badges, flash-in, ring buffer) and the
table kit's per-row alpha.

- [ ] **4b.1** `[core]` `elevation.rs`: `is_elevated()` via `OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY)` + `GetTokenInformation(TokenElevation)`. Add cargo features `Win32_Security` + `Win32_System_Threading`. *AC: returns false in a normal shell, true under "Run as administrator".*
- [ ] **4b.2** `[core]` `ProcEvent { kind: Start|Stop, pid, ppid, name, session_id, sid: Vec<u8>, time_created: u64, exit_status: Option<u32> }` + a `flatten`-free reader that pulls the scalar properties directly (extrinsic events have **no** `TargetInstance`). *AC: unit-tested against a synthetic property map.*
- [ ] **4b.3** `[core]` `ProcessMonitor`: one thread, **two** `exec_notification_query` subscriptions (`SELECT * FROM Win32_ProcessStartTrace`, `…StopTrace`, no `WITHIN`), merged into one `mpsc`. *AC: both streams arrive on one channel with a `kind` tag.*
- [ ] **4b.4** `[core]` Map the denial: `exec_notification_query` returns `WBEM_E_ACCESS_DENIED (0x80041003)` **at subscribe time**, not from the iterator — surface it as a typed `MonitorError::NeedsElevation`, never a raw HRESULT. *AC: the error arm at `monitor.rs:96-99` produces the typed variant.*
- [ ] **4b.5** `[core]` Automatic fallback to the intrinsic `__InstanceCreationEvent WITHIN n` monitor when `NeedsElevation` is returned, with the degraded mode reported to the UI. *AC: the view works unelevated, flagged as degraded.*
- [ ] **4b.6** `[core]` SID → `DOMAIN\user` via `LookupAccountSidW`, with a `HashMap<Vec<u8>, String>` cache (the same handful of SIDs repeat forever). *AC: `S-1-5-18` resolves to `NT AUTHORITY\SYSTEM`; unresolvable SIDs render in SDDL form, not blank.*
- [ ] **4b.7** `[core]` Best-effort `CommandLine`/`ExecutablePath` enrichment: `SELECT CommandLine, ExecutablePath, CreationDate, ParentProcessId FROM Win32_Process WHERE ProcessId = N`, **guarded against PID reuse** by cross-checking `ParentProcessId` + name and converting the event's `TIME_CREATED` (FILETIME, 100 ns since 1601) before comparing with `CreationDate` (CIM_DATETIME) — different epochs *and* representations, so a naive comparison silently no-ops. *AC: a mismatched identity yields `None`, never a wrong command line.*
- [ ] **4b.8** `[core]` Enrichment must not block the monitor thread: it runs on the worker, keyed by event id, and a result that arrives after the row is marked ended still attaches. *AC: a 200 ev/s burst does not stall the stream.*
- [ ] **4b.9** `[gui]` `state/processes.rs`: `TrackedProc { started_at, ended_at: Option<f64>, exit_status, cmdline: Enrichment }` in an insertion-ordered map keyed by `(pid, started_at)` — **not** by pid alone, or PID reuse would overwrite history. *AC: two processes reusing a pid occupy two rows.*
- [ ] **4b.10** `[gui]` Fade-and-**retain**: `alpha = 1.0` while alive; on stop, ease to `PROC_DIM_FLOOR` (0.35) over `PROC_FADE_SECS` (6.0) and **hold** — ended rows are never removed. This deliberately differs from Network's fade-then-drop. *AC: a process that exited 20 minutes ago is still on screen, dimmed.*
- [ ] **4b.11** `[gui]` Retention bounds: `Config.process_max_rows` (default 5,000, oldest ended dropped first — never a live row) + a "Clear ended" button + an optional auto-drop age. *AC: a 12-hour session has bounded memory and the cap is stated in the UI, not silent.*
- [ ] **4b.12** `[gui]` `views/process.rs` table: sign (`+`/`−`) · time · PID · process · user · session · PPID · duration · exit status · command line (ellipsized + tooltip). Started rows use OK, ended rows dim toward neutral; a non-zero `ExitStatus` uses BAD. *AC: sortable on every column; sorting does not disturb the retained ordering when off.*
- [ ] **4b.13** `[gui]` Filters: text (name/user/cmdline), "live only", "ended only", "non-zero exit only", session id. *AC: filters compose.*
- [ ] **4b.14** `[gui]` Degraded-mode banner when running unelevated: states plainly that the intrinsic fallback is in use and that short-lived processes will be missed (measured: ~93% of instant-exit processes), with a "Restart elevated" affordance. *AC: the banner is never shown when the trace subscription succeeded.*
- [ ] **4b.15** `[gui]` Parent-child indent toggle using the free `ParentProcessID` (flat list by default). *AC: toggling does not re-request anything.*
- [ ] **4b.16** `[gui]` Export the process log to CSV/JSON via the shared export menu. *AC: round-trips.*
- [ ] **4b.17** `[infra]` **Elevated verification pass** — the one thing this design rests on that has never been observed. Run elevated, confirm the subscription succeeds, and record: does `ProcessName` carry a path or a bare name; is `Sid` populated; what is the real delivery latency; and what fraction of a 72× instant-exit burst is caught (the intrinsic baseline measured 5/72). *AC: results written into §9, replacing the "unverified" markers.*
- [ ] **4b.18** `[infra]` Release v0.10.1.

### Standing invariants (enforced by `check.ps1`, not one-off tasks)

- [ ] **I1** No `Color32::from_rgb`/`from_gray` outside `theme/tokens.rs`.
- [ ] **I2** No `ui.separator()` (its 6.0 spacing is hardcoded and unthemeable).
- [ ] **I3** No `RichText::strong()` (recolors, does not embolden — use the `ui-med` heading family).
- [ ] **I4** No `Column::auto()` in any virtualized table (widths jitter while scrolling).
- [ ] **I5** No `Context::set_style` (does not exist in 0.35) and no `Context::style()` (does not exist — use `global_style()`).
- [ ] **I6** No `SidePanel`/`TopBottomPanel`/`popup_below_widget`/`menu::bar`/`Context::screen_rect`/`Color32::lerp` — all removed in 0.35.
- [ ] **I7** Every `Panel::left`/`right` that is fixed-width calls `.resizable(false)` **and** `.show_separator_line(false)`.
- [ ] **I8** Every raw px literal in a view comes from `Metrics`, not a constant in the view file.

---

## 9. The Process view — evidence, and what is still unproven

This view is not in the design mock. It comes from two places: a competitive read of
`WMIProcessWatcher.exe` (a 9 KB .NET console tool: `ManagementEventWatcher` over
`Win32_ProcessStartTrace`/`StopTrace`, enriched with `GetOwner` and printed green for
start / red for stop), and a **measured defect in our own event monitor**.

### 9.1 The measured gap

Our monitor's default is intrinsic and polled
(`monitor.rs:19`, `__InstanceCreationEvent WITHIN 2 … Win32_Process`). Measured on
this machine, with a positive control to prove the subscription was healthy:

| configuration | instant-exit processes caught | long-lived control |
|---|---|---|
| `WITHIN 2`, .NET burst | 0 / 20 | 10 / 10 |
| `WITHIN 1`, .NET burst | 0 / 15 | 5 / 5 |
| `WITHIN 2`, `Start-Process` | 0 / 15 | 5 / 5 |
| aggregate over 4 runs, 72 spawns | **5 / 72 caught — 93% missed** | 3 / 3 |

The misses are genuine polling gaps: a catch only happens when a poll boundary lands
inside the process lifetime, so they are **probabilistic, not total** — one run leaked
5 of 20. "An intrinsic subscription never sees short-lived processes" is too strong;
"it misses the large majority of them" is what was measured. Short-lived processes are
exactly what a LOLBin or a dropper looks like, so this is a real blind spot in a
security tool.

### 9.2 What the trace classes actually carry

Verified against the live schema on this machine:

`Win32_ProcessStartTrace` → `ProcessID`, `ParentProcessID`, `ProcessName`,
`SessionID`, `Sid` (raw byte array), `TIME_CREATED`, `SECURITY_DESCRIPTOR`.
`Win32_ProcessStopTrace` → the same **plus `ExitStatus`**.

Two consequences. **(a)** There is no `CommandLine` and no image path — only the bare
executable name — so a command line requires the racy follow-up query in task 4b.7.
**(b)** The owner SID arrives *on the event*, which is strictly better than the .NET
tool's approach: it calls `GetOwner` on a `Win32_Process` instance that may already be
gone. We get the owner with no race at all.

Inheritance: `Win32_ProcessStartTrace → Win32_ProcessTrace → Win32_SystemTrace →
__ExtrinsicEvent`. Extrinsic means provider-pushed: no `WITHIN`, no snapshot polling.

### 9.3 The privilege wall — and the part that is NOT proven

On this machine (a UAC-filtered admin token, no `SeDebugPrivilege`) every subscription
path is denied with **`WBEM_E_ACCESS_DENIED (0x80041003)`** — over WSMan, over DCOM,
and with `SWbemSecurity.Privileges` explicitly requesting `SeDebugPrivilege`. Control
tests establish that the denial is specific rather than a broken session:

| query on the identical connection | result |
|---|---|
| `__InstanceModificationEvent WITHIN 2 … Win32_LocalTime` | OK |
| `__InstanceCreationEvent WITHIN 2 … Win32_Process` | OK |
| `Win32_VolumeChangeEvent` (**also** `__ExtrinsicEvent`-derived) | OK |
| `Win32_ProcessStartTrace` / `StopTrace` / `ThreadStartTrace` / `ModuleLoadTrace` | **AccessDenied** |

So the gate is **not** "extrinsic events need elevation" — `Win32_VolumeChangeEvent` is
extrinsic and subscribes fine unelevated. The gate is the specific provider:
`__EventProviderRegistration` → **"WMI Kernel Trace Event Provider"**, registered for
exactly the five trace classes above. It is also not a `root\CIMV2` namespace ACL,
since other notification queries succeed for the same token.

**What is not established:** that running elevated lifts the denial. No session with an
elevated token was ever available, so the positive case is untested — as is *which*
predicate is operative (high integrity level, `SeDebugPrivilege` in the token, or full
`BUILTIN\Administrators` membership). The `wmi` crate's own doctests annotate these
queries *"This query will fail when not run as admin"*, which is corroboration, not
proof. Task 4b.17 exists to close this, and the design does not depend on the answer:
§4b.5's fallback keeps the view working either way.

Note also that the denial surfaces from `exec_notification_query` itself (synchronous
`ExecNotificationQuery`), i.e. at `monitor.rs:94` and its `Err` arm — not from the
iterator loop. Error handling must sit there.

### 9.4 Why the ended rows stay

Network fades a closed connection over 6 s and then **removes** it. The Process view
deliberately does not: an ended process eases to 35% opacity and stays, carrying its
exit status and lifetime. The question this view answers — "what ran on this box while
I wasn't looking?" — is unanswerable if rows disappear. Memory is bounded by an
explicit row cap and a Clear action (4b.11), never by silent expiry.

### 9.5 What the `wmi` crate cannot do

`wmi 0.18.4` exposes no privilege or moniker path — `grep -rn "Privilege|moniker|
winmgmts"` across its `src/` returns zero hits, and `create_services`
(`connection.rs:290-316`) hardcodes `WBEM_FLAG_CONNECT_USE_MAX_WAIT` with no privilege
string. Any privilege work therefore happens on the **caller's token**, before the
subscription, on the same thread — which WMI then picks up because the crate connects
at `RPC_C_IMP_LEVEL_IMPERSONATE` (`connection.rs:65,169`). All symbols for that
(`OpenProcessToken`, `LookupPrivilegeValueW`, `AdjustTokenPrivileges`, `SE_DEBUG_NAME`,
`TOKEN_ELEVATION`, `TokenElevation`) exist in `windows 0.62.2` and were line-verified;
they need the `Win32_Security` + `Win32_System_Threading` features, which this
workspace does not yet enable.