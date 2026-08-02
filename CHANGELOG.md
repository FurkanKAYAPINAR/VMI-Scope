# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0] - 2026-08-02

### Added
- **Every destination now has a view.** The Explorer is three columns and five
  sub-tabs; Query, Saved, Process, Events, Machines and Compare are real.
- **Multi-host**: a worker per target, responses stamped with the host that
  produced them, real connect and probe timings, and OS/build/UUID from a probe
  that used to be thrown away.
- **Compare** diffs two machines on the class's own key columns.
- **Provider host stats against the WMI host quota** -- the form in which those
  numbers actually mean something.
- A licences panel, a keyboard map, and an empty/loading/error state for every
  view.

### Fixed
- **Seven paths could answer as the current user while the caller believed it
  was talking to a remote host under other credentials.** One of them executed
  a method. There is now exactly one way to reach WMI from the worker, and the
  transport that cannot carry a credential is no longer reachable from any
  request path. Measured before and after with a leak test: 7 answered, then 0
  of 15.
- **The persistence scan reported "nothing found" when it could not look.**
  Every error was swallowed and an empty report returned `Ok` -- in the view
  whose entire purpose is telling those two apart.
- JSON exports reordered the analyst's columns and silently dropped cells from
  ragged rows; the CSV was not rectangular. Both now go through one path.
- A saved query restored its text but not its namespace, so a query saved under
  `root\subscription` ran against `root\CIMV2`.
- The config file was rewritten on every query run.

### Added
- **Every Settings control is now wired, or says why it is not.** Default
  namespace, impersonation level, operation timeout, row limit, byte
  formatting, show-system-classes, the credentials block, the code column guide
  and the window chrome all take effect and persist. Two stay disabled and
  both name the code that would have to change: **Authentication**, because the
  package is fixed per transport in the core (`RPC_C_AUTHN_WINNT` on the
  alternate-credential path, negotiated by the `wmi` crate on the SSO one) and
  there is no second value to offer, and **Monospace font**, because one face is
  embedded and nothing is loaded from disk at runtime by design.
- **Settings → About → Licences**, rendering the bundled OFL and MIT texts
  inline from `include_str!`. The obligation is to distribute the licence *with*
  the software, so the text is compiled into the binary rather than read from a
  file beside it.
- **A keyboard map (F1**, or the status bar's `F1 keys` button**)**, generated
  from the same binding table `handle_shortcuts` dispatches from, so it cannot
  drift from what the keys do.
- **A close gate.** Closing the window while a method invocation is in flight is
  answered with `CancelClose` and a confirmation that says plainly that the call
  has already been sent, cannot be cancelled, and will lose its result. Every
  other pending operation is a read; this one is a write.
- **A perf harness**, `vmiscope --bench`. Measured on this machine, release
  build, 240 frames a scenario with the first 60 discarded: a 50,000-row result
  table costs **0.89 ms/frame mean, 1.24 ms p95**; a 1,400-class list **0.58 ms
  mean, 0.76 ms p95**; a 2,000-event process stream **1.19 ms mean, 1.50 ms
  p95** — all against a 16.67 ms 60 fps budget. The frame *interval* sat at
  13.3 ms throughout, which is the display period and not a measurement of this
  code.
- Empty states for the Network table and the Explorer class list, which drew a
  bare header over nothing, and a status line for Saved, Machines and Settings,
  which the status bar had been calling "not built yet" since before they were
  built.

### Changed
- **File dialogs and config writes no longer block the frame loop.** `rfd`'s
  dialogs do not return until the user is finished with them, which froze the
  whole application — live pollers included — for as long as somebody browsed
  their filesystem. Both now run on an IO thread and report back once a frame.
  A failed write reaches the error log instead of a discarded `Result`, and a
  completed save says where it went.
- **The focus ring is painted once per frame** for whatever holds focus, read
  from `Memory::focused`, instead of each widget remembering to ask. The audit
  that prompted it found eleven raw controls — three `ui.checkbox`, eight
  `ui.selectable_label` — with no ring at all.
- Segmented controls no longer render their options backwards inside a
  right-to-left value column. `On | Off` was reading as `Off | On`, and
  Identify / Impersonate / Delegate — an ascending scale of what you give away —
  ran the other way.
- `cargo deny check` passes. `BSL-1.0` is allowed (Boost, permissive, and its
  notice requirement explicitly excludes object code); RUSTSEC-2026-0192 is
  ignored with a written reason — `ttf-parser` is *unmaintained* rather than
  vulnerable, has no upgrade, and reaches this workspace only through winit's
  Wayland client-side decorations, which never run on Windows.

### Decided
- **`egui_extras`'s `serde` feature stays off** (task 7.11). It switches the
  table's column widths from `get_temp` to `get_persisted` — and that changes
  nothing here, because `eframe`'s `persistence` feature is off (`ron` is not in
  `Cargo.lock`), so egui's memory is never serialised to disk and the widths are
  lost on restart either way. Making it reachable would mean turning on
  `eframe/persistence`, which writes a second preferences file beside
  `config.json` with its own lifecycle, no version field and no migration —
  against a `Config` that has all three. Column widths are also keyed by
  `id_salt`, so a persisted width would outlive the column set it was measured
  for and silently mis-lay a table that had gained a column.

### Fixed
- The Network table sorted on text that was not on screen: a UDP row showed `*`
  for its remote port and sorted on `""`, and a state-less row showed an em dash
  and sorted on `""`. Cell text and sort key are now one function.
- Under a column sort, the Network table's tie-break was `HashMap` iteration
  order, so rows sharing a value could reshuffle between snapshots — the one
  thing a fading row must not do.
- A stale error could sit in the status bar for the rest of the session. Only a
  successful *query* used to clear it; any successful reply does now, and the
  error log still keeps the history.
- The Process view's Time column is a **UTC wall clock**. `ProcEvent::time_created`
  is a real FILETIME that was consumed by the core's pid-reuse guard and dropped,
  so the column could only say "T+MM:SS since the app started" — the wrong axis
  for "what ran at 03:14". A row with no reported creation time falls back to the
  `T+` form, dimmed and labelled, rather than inventing a date.
- Removed `widgets::table::sortable_header`, dead since the last hand-rolled
  table went away and kept alive only by a module-level `allow(dead_code)`.
  Taking that allow off found nine more unused items, all now gone.

## [0.8.0] - 2026-08-01

### Added
- **A real application shell.** The five-tab strip is gone. An undecorated
  window with a 40px title bar (app mark, machine chip, palette trigger, live
  toggle, and its own minimise / maximise / close), a 64px rail carrying all
  eleven destinations in three groups, and a 24px status bar. `--decorated`
  restores the OS caption for anyone who wants it.
- **A Ctrl+K command palette** over every destination plus Refresh, Run query,
  Toggle live, Export and the theme switches, with class, property and method
  hits from the existing search index. Arrow keys move, Enter runs, Esc closes.
- **Settings**, in four groups. Accent, density, live polling, default script
  language, row limit and operation timeout take effect immediately; everything
  else renders disabled with a tooltip naming what will wire it, because an
  enabled control that does nothing is worse than no control.
- **Process monitoring in the core**: `Win32_ProcessStartTrace` / `StopTrace`
  where the token allows it, falling back to the polled query where it does
  not, and saying which mode it is in. The row model keeps ended processes on
  screen, dimmed, rather than dropping them.

### Changed
- `Config` is versioned. A v1 file loads, keeps its history and saved queries,
  and is migrated in place.
- A maximised undecorated window no longer hangs 8 points off every screen
  edge, which had been putting the top of the title bar and the outer edge of
  the close button off the display.

## [0.7.0] - 2026-07-27

### Added
- **The Nocturne design system.** A dark ground, one accent (steel, with teal
  and amber alternates), Inter for text and JetBrains Mono for every value,
  path and identifier, and the Phosphor icon set -- all embedded, nothing
  fetched at runtime. Outlined buttons rather than filled ones, table rows
  separated by a rule that fades at each end, a 2px accent focus ring.
- **A widget kit**: one virtualised, sortable, selectable data table replacing
  four hand-rolled ones; a syntax-tinted code panel for generated scripts and
  MOF; cards, chips, fields, key/value grids and waiting states. Every view now
  draws through it, so the look cannot drift view by view.
- **Class reflection**: every class-level qualifier, the `__Derivation` ancestry
  chain, and a `ClassKind` bit set (Dynamic / Association / Event / System /
  Abstract / Singleton / Perf) on `ClassSchema`, so the Explorer can badge and
  filter a class without a second round trip.
- **Parameter direction**: a method's in- and out-signatures are merged into one
  list, so a parameter appears once carrying `[in]`, `[out]` or `[in/out]`
  instead of being silently duplicated.
- **A design-system gate** (`check.ps1`) enforcing what a compiler cannot: no
  colour literal outside the token module, no unthemeable separator, no glyph
  the embedded fonts cannot render.
- **Documentation**: [docs/REDESIGN.md](docs/REDESIGN.md), the phased plan with
  every egui 0.35 API checked against the pinned sources, and
  [docs/FINDINGS.md](docs/FINDINGS.md), the WMI and egui behaviour this work had
  to establish by measurement.

### Changed
- Queries are bounded by a row cap **and** a deadline, and a partial result
  always says which. A cap alone is not enough: `CIM_DataFile` yields no rows at
  all for at least twelve seconds, so there is never anything to count.
- Cancelling a query now lands in about 10 ms, and closing the window during one
  exits in about 600 ms instead of hanging.
- **Method invocation is more robust**: `is_static` now also covers a class with
  no `Key` property or `Singleton = TRUE`, because WMI omits the `Static`
  qualifier far more often than not (`Win32_OperatingSystem` carries it on none
  of its five methods). An instance-path call WMI rejects is retried against the
  class path.
- `default_fonts` is off on egui and eframe. Shipping three real faces and
  dropping the four egui embeds is a net +159 KiB, not +1.5 MB.
- `crates/vmiscope-gui/src/app.rs` split from 2,973 lines into 28 modules, then
  reskinned. The split itself was pure code motion, verified item by item.

### Fixed
- **A third of the icon set rendered as unrelated letters.** The icon font was a
  fallback of the text families, and Inter carries 745 Private Use Area glyphs
  of its own that answered 32 of ours first -- a download arrow rendered as `S`
  with a caron. Icons now have their own family.
- **The confirmation gate could invoke a method it had not described.** If the
  class schema was unavailable when the dialog was confirmed, it assumed the
  method was static and took no arguments, then invoked it against the class
  path with every typed argument dropped. For a method like `Terminate` that is
  a different operation from the one the user agreed to. It now declines.
- Stopping the event monitor left its error message on screen, reporting a
  subscription that no longer existed.

## [0.6.0] - 2026-07-25

### Added
- **Persistence hunter — orphan detection & wider scan**: surfaces staged-but-
  unbound `__EventFilter`/`__EventConsumer` (a binding-only-evasion technique)
  and scans `root\default` in addition to `root\subscription`.
- **External-connection flags** (Network tab): established TCP to public IPs is
  counted, filterable ("external only"), and globe-marked — possible C2/exfil.
- **Snapshot & diff for Providers** (host-process changes over a baseline).
- **Persistent query history + saved queries** (`%APPDATA%\VMI-Scope\config.json`).
- **Light/dark theme switch**; **event-log JSON export**; **error log panel**
  (errors accumulate instead of overwriting); **audit log** of every mutating
  method call (`audit.log`).

### Changed
- CI enforces `clippy -D warnings` and runs **cargo-deny** (advisories + licenses).

## [0.5.0] - 2026-07-25

### Added
- **Alternate credentials for remote hosts** (experimental): connect to a remote
  machine with an explicit user / password / domain via raw DCOM
  (`COAUTHIDENTITY` + re-blanketed enumerator proxies). Browse/query, Network,
  and Providers route over the credentialed connection; Explorer schema/methods/
  MOF and the persistence consumer enrichment stay current-user. **Compile- and
  credential-plumbing-verified** (a bogus-cred local connect is correctly
  rejected by WMI, proving credentials reach DCOM with no memory issue); the
  remote *query* path is unverified against a live remote host, since WMI forbids
  credentialed local connections and it cannot be tested on one machine.

### Changed
- `vmiscope-core` is now crates.io-publishable (metadata + crate README);
  `cargo publish` just needs a token.

## [0.4.0] - 2026-07-25

### Added
- **Live event monitor** (Events tab): run a WMI notification query and watch
  events stream in — default watches process creation
  (`__InstanceCreationEvent WITHIN 2 ... Win32_Process`). Runs on its own thread
  so it never blocks the UI; drills into the embedded `TargetInstance`.
- **Snapshot & diff** for the Persistence hunt: save a baseline and diff the
  current scan against it (added / changed / removed subscriptions).
- **HTML report** for the persistence scan — a self-contained, styled artifact
  (risk colours + MITRE ATT&CK T1546.003).

## [0.3.0] - 2026-07-25

### Added
- **Export** — save query results and the persistence report to **CSV** or
  **JSON** via a native save dialog.
- **Unit tests** for the core scoring/parsing logic (risk assessment, path
  parsing, variant conversions, CIM-type classification, table alignment);
  `cargo test` now runs in CI.
- **CodeQL** code-scanning workflow and **Dependabot** dependency updates
  (cargo + GitHub Actions); GitHub Discussions enabled.

### Changed
- Professionalized the README header (centered badge layout).

## [0.2.0] - 2026-07-24

### Added
- **Remote host connection** (top bar): point every tab at a remote machine by
  hostname/IP, connecting as the **current user (SSO)**. All views — Explorer,
  Network, Persistence, Providers — transparently reflect the remote host.
  (Alternate credentials need a raw-DCOM layer and are tracked separately.)
- **Global search** (Explorer left panel): build a name index for the current
  namespace, then search across **class, property, and (optionally) method
  names**. Clicking a hit jumps to the class — a property hit runs
  `SELECT <prop> FROM <class>`, a method hit opens the Actions panel on it.
- **Method execution** (Explorer → ⚙ Actions): invoke any WMI method with a
  dynamic, type-aware parameter form. Static methods (e.g. `Win32_Process.Create`)
  and instance methods (pick a target from a loaded instance list) are both
  supported. **Every invocation is gated behind an explicit confirmation modal**
  that restates the target and arguments, with an extra warning for
  destructive-looking method names. Introspection reflects the raw
  `IWbemClassObject`; execution uses the `wmi` crate's `exec_method`.
- **MOF viewer** — a `📄 MOF` button shows the MOF (Managed Object Format) text
  of the selected class/instance in a floating, copyable window
  (`IWbemClassObject::GetObjectText`).
- **Reflective class schema view** (Explorer → Schema toggle): for any class —
  even zero-instance ones — shows every property with its CIM type, key/read/
  write flags, units, description, and `ValueMap` enum (on hover), plus each
  method's in/out parameter signatures. Built by reflecting over the raw
  `IWbemClassObject` (`GetPropertyQualifierSet`, `BeginMethodEnumeration`) via
  the `windows` crate.
- **Class-list caching** — revisiting a namespace no longer re-enumerates.
- **Providers tab**: lists WMI providers (`Msft_Providers`) with the process
  hosting each — provider, namespace, host PID, host process name, hosting
  group. Sortable; useful for troubleshooting `wmiprvse.exe`.
- **Persistence tab (WMI event-subscription hunter)**: enumerates
  `root\subscription` — `__EventFilter`, all `__EventConsumer` subclasses, and
  `__FilterToConsumerBinding` — correlates each binding to its trigger and
  action, and scores it Low/Medium/High for how much it looks like fileless
  persistence (MITRE ATT&CK T1546.003). Sortable, colour-coded, with the filter
  query and action on hover.
- **Explorer tab**: lazy namespace tree, class browser with live filtering,
  WQL editor (Ctrl+Enter), auto-generated `SELECT *` for a selected class,
  virtualized result grid, and a per-row detail panel.
- **Network tab**: live TCP/UDP connection monitor sourced from
  `MSFT_NetTCPConnection` / `MSFT_NetUDPEndpoint` joined to `Win32_Process`,
  with 1.5s auto-refresh, pause, colour-by-state, and fade-out of closed
  connections.
- **Sortable tables** everywhere — click any column header to sort
  (ascending → descending → unsorted), numeric-aware.
- **Script generation** — PowerShell and VBScript for the current query.
- Background WMI worker thread so the UI never blocks.

[Unreleased]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/releases/tag/v0.2.0
