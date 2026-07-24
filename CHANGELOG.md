# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/FurkanKAYAPINAR/VMI-Scope/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/FurkanKAYAPINAR/VMI-Scope/releases/tag/v0.2.0
