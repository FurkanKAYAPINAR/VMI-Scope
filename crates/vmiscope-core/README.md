# vmiscope-core

The GUI-agnostic **WMI engine** behind [VMI-Scope](https://github.com/FurkanKAYAPINAR/VMI-Scope).

Where the [`wmi`](https://crates.io/crates/wmi) crate is optimized for *typed*
queries, `vmiscope-core` fills the *generic, reflective* gap — the operations a
WMI explorer needs:

- Enumerate namespaces and classes; run arbitrary WQL.
- **Reflect a class schema** — property CIM types, qualifiers (`Description`,
  `ValueMap` enums, key/read/write), and method signatures — even for
  zero-instance classes (raw `IWbemClassObject` reflection).
- **Execute methods** dynamically (`Win32_Process.Create`, `StdRegProv`, …).
- Read **MOF** text (`GetObjectText`).
- A live **network** table, **event-subscription** (persistence) scan with risk
  scoring, **provider → host process** mapping, and a background **event
  monitor**.
- Export to CSV / JSON / HTML; diff a scan against a baseline.

All WMI work is driven through a background [`WmiWorker`] over channels, so
callers never block. **Windows only** (DCOM/COM).

```rust
use vmiscope_core::{Request, Response, WmiWorker};

let worker = WmiWorker::spawn();
worker.send(Request::Query {
    id: 1,
    namespace: "root\\CIMV2".into(),
    wql: "SELECT Caption, Version FROM Win32_OperatingSystem".into(),
});
for resp in worker.poll() {
    if let Response::QueryResult { result, .. } = resp {
        println!("{:?}", result.rows);
    }
}
```

## License

MIT
