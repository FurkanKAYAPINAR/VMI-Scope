# VMI-Scope

[![CI](https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/ci.yml/badge.svg)](https://github.com/FurkanKAYAPINAR/VMI-Scope/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6)

A fast, **security-focused WMI explorer** written in Rust.

VMI-Scope browses the Windows Management Instrumentation tree the way a class
browser browses code: expand namespaces, list classes, inspect instances, and
run raw WQL — all from a native, non-blocking GUI. Unlike the classic C#/.NET
explorers, the core is a reusable Rust crate built on the reflective WMI/COM API,
filling a real gap in the Rust ecosystem (the `wmi` crate is optimized for
*typed* queries; VMI-Scope does *generic* reflection).

> Status: **v0.1 — Milestone 1 (working explorer).** See the [Roadmap](#roadmap).

## Features

Two tabs: **Explorer** (browse WMI) and **Network** (live connection monitor).

### Explorer
- **Namespace tree** — lazily enumerated from `root` down (`__NAMESPACE`).
- **Class browser** — every class in the selected namespace (1400+ in
  `root\CIMV2`), with live filtering.
- **Instance viewer** — click a class to auto-generate `SELECT * FROM <Class>`
  and see the instances in a virtualized table (smooth with thousands of rows).
- **WQL editor** — run arbitrary queries (Ctrl+Enter), against any namespace.
- **Row detail** — click any row to see every property as a `name → value` grid.

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
- [x] Filter classes and instances
- [x] Virtualized result grid + per-row detail
- [x] **Live network monitor** (process / IP / port / state, with fade-on-close)

**Next (WMI Explorer parity)**
- [ ] **Reflective schema view** — property types, qualifiers (descriptions,
      `ValueMap` enums), and method signatures for any class, even with zero
      instances (via `IWbemClassWrapper` + `GetQualifierSet`).
- [ ] **Method execution** — dynamic parameter form + `ExecMethod`
      (e.g. `Win32_Process.Create`).
- [ ] **MOF view** for the selected class/instance.
- [ ] **Script generation** — PowerShell + VBScript for the current query/class.
- [ ] **Global search** across class / method / property names.
- [ ] **Alternate credentials + remote host** connections.
- [ ] **Async vs sync** enumeration toggle; class/instance **caching**.
- [ ] **WMI provider process info**.

**Beyond parity (security)**
- [ ] **Event-subscription hunter** — enumerate `root\subscription`
      (`__EventFilter` / `__EventConsumer` / `__FilterToConsumerBinding`) to
      surface WMI persistence — a favourite fileless technique.
- [ ] **Live event monitor** — async notification queries.
- [ ] Flag suspicious connections/persistence inline.

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
