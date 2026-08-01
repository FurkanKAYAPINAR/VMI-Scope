# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/releases/tag/v0.2.0
