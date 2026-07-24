# Contributing to VMI-Scope

Thanks for your interest! VMI-Scope is a young project with a clear roadmap and
lots of well-scoped work, so it's an easy codebase to jump into.

## Getting started

VMI-Scope targets **Windows** (it talks to WMI/COM). You need a stable Rust
toolchain with the MSVC target:

```powershell
rustup default stable            # x86_64-pc-windows-msvc
git clone https://github.com/FurkanKAYAPINAR/VMI-Scope.git
cd VMI-Scope
cargo run -p vmiscope-gui --release
```

To check the engine without the GUI:

```powershell
cargo run -p vmiscope-core --example probe
```

## Project layout

```
crates/
├─ vmiscope-core/   # GUI-agnostic WMI engine
│  ├─ worker.rs     #   background thread: Request → Response over channels
│  ├─ network.rs    #   live TCP/UDP connection model
│  ├─ value.rs      #   wmi::Variant conversions
│  └─ examples/probe.rs
└─ vmiscope-gui/    # egui / eframe front-end
   ├─ app.rs        #   state, request/response plumbing, all panels
   └─ main.rs       #   window boot
```

Two rules keep the design clean:

1. **The core never depends on the GUI.** All WMI access lives in
   `vmiscope-core` and is exposed as plain data types. If you can't test it from
   `examples/probe.rs`, it probably belongs in the GUI instead.
2. **The UI never blocks.** Every WMI call goes to the worker thread as a
   `Request` and comes back as a `Response`; the UI drains replies once per
   frame. Never call WMI directly from `app.rs`.

## Coding conventions

- Format with `cargo fmt --all` before committing (CI enforces it).
- Keep `cargo clippy --all-targets` clean.
- Match the surrounding style: small focused functions, doc comments on public
  items, and comments that explain *why*, not *what*.

## Submitting changes

1. Fork and create a topic branch.
2. Make your change; add or update `examples/probe.rs` coverage for core changes.
3. Run `cargo fmt --all`, `cargo clippy --all-targets`, and `cargo build --all`.
4. Open a PR describing the change and linking any related issue.

## Where to start

The [Roadmap](README.md#roadmap) lists the open features. Good first tasks:

- **MOF view** for a selected class (`GetObjectText`).
- **Global search** across class names.
- More **WQL editor** niceties (history, syntax highlighting).
- Extra columns / grouping in the **Network** tab.

Bigger, high-impact items (great if you know some COM): reflective schema view
with qualifiers, method execution, and the WMI event-subscription hunter.

## Reporting security issues

Please use a private
[security advisory](https://github.com/FurkanKAYAPINAR/VMI-Scope/security/advisories/new)
instead of a public issue.
