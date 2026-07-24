<h1 align="center">VMI-Scope</h1>

<p align="center">
  A fast, <strong>security-focused WMI explorer</strong> for Windows, written in Rust.
</p>

<p align="center">
  <a href="https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/ci.yml"><img src="https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/codeql.yml"><img src="https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/codeql.yml/badge.svg" alt="CodeQL" /></a>
  <a href="https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/dependabot/dependabot-updates"><img src="https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/dependabot/dependabot-updates/badge.svg" alt="Dependabot Updates" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT" /></a>
  <img src="https://img.shields.io/badge/platform-Windows-0078D6?logo=windows&logoColor=white" alt="Platform: Windows" />
</p>

<p align="center">
  <a href="https://github.com/FurkanKAYAPINAR/VMI-Scope/releases/latest"><img src="https://img.shields.io/github/v/release/FurkanKAYAPINAR/VMI-Scope?color=success" alt="Release" /></a>
  <a href="https://github.com/FurkanKAYAPINAR/VMI-Scope/stargazers"><img src="https://img.shields.io/github/stars/FurkanKAYAPINAR/VMI-Scope?style=flat&logo=github" alt="Stars" /></a>
  <a href="https://github.com/FurkanKAYAPINAR/VMI-Scope/discussions"><img src="https://img.shields.io/github/discussions/FurkanKAYAPINAR/VMI-Scope" alt="GitHub Discussions" /></a>
  <a href="https://github.com/FurkanKAYAPINAR/VMI-Scope/issues"><img src="https://img.shields.io/github/issues/FurkanKAYAPINAR/VMI-Scope" alt="Issues" /></a>
  <img src="https://img.shields.io/badge/rust-1.75%2B-orange?logo=rust&logoColor=white" alt="Rust" />
</p>

VMI-Scope browses the Windows Management Instrumentation tree the way a class
browser browses code: expand namespaces, list classes, inspect instances, and
run raw WQL — all from a native, non-blocking GUI. Unlike the classic C#/.NET
explorers, the core is a reusable Rust crate built on the reflective WMI/COM API,
filling a real gap in the Rust ecosystem (the `wmi` crate is optimized for
*typed* queries; VMI-Scope does *generic* reflection).

> Status: **v0.2 — feature-complete explorer + security tooling.** See the [Roadmap](#roadmap).

## Features

Four tabs: **Explorer** (browse WMI), **Network** (live connections),
**Persistence** (WMI event-subscription hunter), and **Providers**.
Every table sorts — click a column header (ascending → descending → off).
Point the **Host** field in the top bar at a remote machine (current-user SSO)
and every tab reflects it.

### Explorer
- **Namespace tree** — lazily enumerated from `root` down (`__NAMESPACE`).
- **Class browser** — every class in the selected namespace (1400+ in
  `root\CIMV2`), with live filtering + **caching** on revisit.
- **Instance viewer** — click a class to auto-generate `SELECT * FROM <Class>`
  and see the instances in a virtualized, sortable table.
- **Schema view** — properties (CIM types, key/read/write, `ValueMap` enums) and
  method signatures for any class, even zero-instance ones.
- **Method execution** — invoke any method with a type-aware parameter form,
  behind a confirmation gate.
- **MOF view** — the MOF text of any class/instance.
- **Global search** — across class / property / method names.
- **WQL editor** — run arbitrary queries (Ctrl+Enter), against any namespace.
- **Row detail** — click any row to see every property as a `name → value` grid.
- **Script generation** — PowerShell / VBScript for the current query.

### Network (live)
- **Real-time connection table** sourced from WMI — every TCP connection and UDP
  endpoint with **protocol, local IP:port, remote IP:port, state, PID, and
  process name** (`MSFT_NetTCPConnection` / `MSFT_NetUDPEndpoint` joined to
  `Win32_Process`).
- **Auto-refresh** every 1.5s, with Pause and manual Refresh.
- **Fade on close** — active connections are shown in full colour (green =
  established, blue = listening, amber = transitional); when a connection closes
  it stays in place and **fades out over ~6s** instead of vanishing, so you can
  see what just happened.
- **Filter** by process, IP, port, or state.

### Persistence (WMI event-subscription hunter)
- Enumerates `root\subscription` — `__EventFilter`, every `__EventConsumer`
  subclass, and `__FilterToConsumerBinding` — and **correlates each binding** to
  its trigger (WQL query) and action (command line / script / target).
- **Risk scoring** (Low / Medium / High) flags what looks like fileless
  persistence — CommandLine/ActiveScript consumers, encoded payloads, intrinsic
  event triggers (MITRE ATT&CK **T1546.003**).

### Providers
- Lists WMI providers (`Msft_Providers`) and the **process hosting each** —
  provider, namespace, host PID, host process name, hosting group. Handy for
  chasing a runaway `wmiprvse.exe`.

### Everywhere
- **Never blocks** — all WMI work runs on a background thread; the UI stays at
  60 fps even while snapshots, namespace, or class lists are loading.

## Architecture

A two-crate Cargo workspace that keeps WMI access decoupled from the UI, so the
front-end could later be swapped (e.g. Tauri) without touching the engine.

```
vmi-scope/
├─ crates/
│  ├─ vmiscope-core/     # GUI-agnostic WMI engine
│  │  ├─ worker.rs       #   background thread: Request → Response over channels
│  │  ├─ value.rs        #   wmi::Variant → display string
│  │  └─ examples/probe.rs   # CLI reality-check against live WMI
│  └─ vmiscope-gui/      # egui / eframe desktop front-end
│     ├─ app.rs          #   state, request/response plumbing, all panels
│     └─ main.rs         #   window boot
```

**Threading model.** COM apartments are thread-affine and `wmi`'s
`WMIConnection` is `!Send`, so a single dedicated worker thread owns every
connection. The GUI communicates purely through channels: it pushes typed
`Request`s (each with a monotonic id) and drains `Response`s once per frame.
Stale replies from superseded queries are dropped by id.

**Reflective access.** For a *generic* explorer we can't deserialize into known
structs. Class enumeration and (soon) schema introspection use the low-level
`WMIConnection::exec_query` → `IWbemClassWrapper` path, which exposes
`.class()`, `.path()`, `.list_properties()`, and `.get_property()` — including
WMI system properties (`__CLASS`, `__PATH`) that the generic `HashMap` path
hides.

## Build & run

Requires a stable Rust toolchain with the MSVC target (`x86_64-pc-windows-msvc`).

```powershell
# Clone
git clone https://github.com/FurkanKAYAPINAR/VMI-Scope.git
cd VMI-Scope

# Run the GUI
cargo run -p vmiscope-gui --release

# Sanity-check the engine against live WMI (prints namespaces, classes, a query)
cargo run -p vmiscope-core --example probe
```

## Roadmap

The goal is to match the classic [WMI Explorer](https://github.com/vinaypamnani/wmie2)
feature set, then go past it with security tooling. Status:

**Done**
- [x] Browse namespaces / classes / instances in one view
- [x] Run WQL queries + auto-generate `SELECT *` for a selected class
- [x] Filter classes and instances; **sortable** tables everywhere
- [x] Virtualized result grid + per-row detail
- [x] **Live network monitor** (process / IP / port / state, with fade-on-close)
- [x] **Script generation** — PowerShell + VBScript for the current query
- [x] **Event-subscription hunter** — `root\subscription` persistence scan with
      risk scoring (MITRE ATT&CK T1546.003)
- [x] **WMI provider process info** (`Msft_Providers` → host process)
- [x] **Reflective schema view** — property types, qualifiers (descriptions,
      `ValueMap` enums), and method signatures for any class, even zero-instance
      ones (raw `IWbemClassObject` reflection).
- [x] **Method execution** — dynamic, type-aware parameter form + confirm gate
      (`Win32_Process.Create`, `StdRegProv`, …).
- [x] **MOF view** for the selected class/instance.
- [x] **Global search** across class / property / method names.
- [x] **Class-list caching** (async is inherent via the background worker).
- [x] **Remote host** connection (current-user SSO).
- [x] **Export** query results & persistence report to CSV / JSON.
- [x] **Unit tests** + CI (`cargo test`, CodeQL, Dependabot).

**Next**
- [ ] **Alternate credentials** for remote hosts (needs a raw-DCOM
      `COAUTHIDENTITY` layer — the `wmi` crate's credentialed path runs queries
      as the local user).
- [ ] **Snapshot & diff** of Persistence/Providers over time (baseline hunting).
- [ ] **HTML report** for the persistence scan (analyst-ready artifact).
- [ ] **Live event monitor** — async notification queries.
- [ ] Mockable `WmiBackend` trait → publish `vmiscope-core` to crates.io.
- [ ] Flag suspicious connections inline.

> Note: per-connection *throughput* (bytes/sec) isn't exposed by WMI; that would
> need an ETW/`iphlpapi` source and is tracked separately.

## Why Rust

The reflective WMI/COM path is where Rust's WMI ecosystem is thinnest — building
it here is both the deepest way to learn WMI/COM internals and a genuinely novel
open-source contribution, rather than re-implementing an existing C# tool.

## Contributing

Contributions are very welcome — this is a young project with a clear roadmap and
plenty of self-contained tasks. See [CONTRIBUTING.md](CONTRIBUTING.md) for how to
build, the architecture, coding conventions, and where to start. By participating
you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

VMI-Scope is a **read-oriented** WMI tool. Some planned features (method
execution, remote connections) can change system state or reach other hosts — use
them only where you are authorized to. Please report vulnerabilities through a
private [security advisory](https://github.com/FurkanKAYAPINAR/VMI-Scope/security/advisories/new)
rather than a public issue.

## License

MIT — see [LICENSE](LICENSE).
