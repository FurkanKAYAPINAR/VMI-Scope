# VMI-Scope — Engineering Analysis (v0.9.0, commit e9cf84a)

Rust WMI/COM explorer. Workspace: `vmiscope-core` (~10.5k LoC reflective WMI engine) + `vmiscope-gui` (~22k LoC egui/eframe front end). Every finding cites `file:line` against sources actually read.

## 1. Code Quality

**Architecture — strong.** COM apartments are thread-affine, so all WMI work is confined to one worker thread, one target, reached only over `mpsc` channels (`worker.rs:1-20, 515-567`). Cancellation uses an out-of-band flag plane raised *before* the message is queued (`WorkerControl` `enumerate.rs:122-193`; `WmiWorker::send` `worker.rs:546-553`), so a `Cancel` never waits behind the query it stops. The keystone is the **"one door" `bind()` dispatcher** (`worker.rs:1128-1152`): every WMI op obtains its connection through one function, and the `Bound` enum (`enumerate.rs:551-621`) makes picking the wrong (wrong-principal) transport structurally impossible — it documents that this replaced seven paths that silently ran as the current user under alternate creds.

**`unsafe`/COM FFI soundness — excellent.** 50 unsafe sites in core, all reviewed sound. RAII wrappers (`BSTR`/`VARIANT`/`HSTRING`/`PWSTR`) free via `Drop`; each `VARIANT::default()` is re-declared per loop iteration so nothing leaks (`reflect.rs:86-97`, `remote.rs:188-211`). The dangling-pointer trap is handled correctly — `wide()` returns `(HSTRING, PCWSTR)` and callers bind `let (_h, pcw)` to keep the string alive (`reflect.rs:67-72`, used `:122,:193,:210,:384`). **SID handling is textbook** (`sid.rs`): untrusted SID bytes are structurally validated before FFI (`:129-135`), re-homed into a `u32`-aligned buffer (`:91-101`), re-checked with `IsValidSid` (`:111`), and the `ConvertSidToStringSidW` allocation freed with `LocalFree` (`:203-205`). `is_elevated` closes the token on all paths and only trusts a full struct write (`elevation.rs:52-53`). `PinnedAuth` correctly boxes `COAUTHIDENTITY` + its buffers for pointer stability (`remote.rs:80-115`).

**Test coverage — the central gap.** Unit tests are plentiful but cover *pure logic only* (assoc_rows, settle/next_batch, variant/SID conversions, perf_wql). **No test exercises the COM/FFI path** (`DirectConn`/`RemoteConn`/`drain`); CI's `cargo test` on windows-latest runs the same pure tests. The alt-cred remote transport is explicitly "Unverified against a live remote host" (`remote.rs:8-19`). The riskiest code has the least coverage.

**Tech debt.** `worker.rs` is a 2,664-line god-module with five pieces of mutable `thread_local` state (`:1041-1065`) every helper reads. GUI has ~125 unwrap/expect/index sites vs core's 2 (both thread-spawn `expect`, acceptable) — confirmed these are startup-invariant assertions or locally-guarded, not user-triggerable crashes. No `TODO`/`FIXME` markers anywhere. CI is strict: fmt-check, clippy `-D warnings`, cargo-deny, CodeQL, Dependabot.

## 2. Security Audit

**2.1 Command-execution surface (highest impact).** `invoke_method` (`method.rs:221-275`) executes *any* WMI method via `ExecMethod` — `Win32_Process.Create`, `StdRegProv` writes, service control, shutdown. The worker runs any `Request::InvokeMethod` unconditionally (`worker.rs:941-976`). The **only** guard is the GUI: a two-step arm→confirm gate (`invoke.rs:271-302`, sole `fire_invoke`→`request_invoke`→`Request::InvokeMethod` chain), which disarms on any edit (`:226-228`) or method switch (`:341-346`) and starts disarmed (`:83`). Every invocation is audited via `config::append_audit` before send (`requests.rs:280-294`). **Findings:** (a) the guard is entirely client-side — the **core crate has no allow/deny-list**, so any library consumer gets an unguarded method-execution API; (b) `is_dangerous_method` (`util.rs:9-29`) is substring-based (over-matches "set"/"change", can under-match provider verbs) and only escalates a *warning label* (`invoke.rs:251-261`), never blocks.

**2.2 WQL injection.** Arbitrary WQL is the product feature (`run_query` `worker.rs:1622-1658`; Query view `query.rs:332`→`requests.rs:315-333`; Compare `format!("SELECT * FROM {}", self.class.trim())` `compare.rs:334`) — intended, bounded by cap/deadline/cancel. Structured paths are safe: instance listing/counting use `CreateInstanceEnum` not WQL (`method.rs:120-122`, `enumerate.rs:473-485`), and class names for Associations/Schema/Instances come from enumerated lists, never free-typed (`requests.rs:383-393`, `classlist.rs:157-175`). **The one genuine injection-shaped gap:** `class_associations` interpolates a class name into WQL unescaped — `format!("REFERENCES OF {{{subject}}} WHERE SchemaOnly")` (`worker.rs:1557`) and `ASSOCIATORS OF` (`:1569`); a `}` in `subject` breaks out of the object path. Read-only `SchemaOnly` and identifier-only input make real-world risk low, but it is unvalidated interpolation (Bug #1). Minor: events monitor doesn't escape the single-quote in `ISA '{class}'` (`detail.rs:158-161`).

**2.3 COM lifetime/leaks.** None found. `FORWARD_ONLY | RETURN_IMMEDIATELY` (`enumerate.rs:333`), objects released as `Option` slots drop per batch (`:280-290`), `BeginEnumeration`/`EndEnumeration` always paired.

**2.4 Impersonation.** Reaches WMI on both transports via hand-called `CoSetProxyBlanket` (`remote.rs:124-140`, `enumerate.rs:623-639`), measured/documented (`host.rs:80-117`). `Delegate` (third-machine credential reach) is user-selectable and flagged dangerous but offered without extra friction.

**2.5 Credential handling — mostly excellent, two gaps.** Never persisted (test `a_target_never_serializes_a_password` `config.rs:1305-1329`; only `CredRef` who-not-secret is stored `config.rs:240-293`), redacted in `Debug` (`remote.rs:53-61`), excluded from the identity hash key (`host.rs:24-68`, test `:239-246`), masked UI field (`machines.rs:560-574`), app struct doesn't derive `Debug`. **Gap 1:** `Credential.password` is a plain `String` (`remote.rs:47-51`) and `PinnedAuth` buffers (`:88-114`) are never zeroized — cleartext lingers in heap for the session. **Gap 2:** the audit log records method **arguments in plaintext** (`requests.rs:281-294`); a secret passed as a method argument lands in `audit.log` cleartext.

**2.6 Dependencies — clean.** 444 crates; `windows 0.62.2`, `wmi 0.18.4`, `egui/eframe 0.35.0`, `serde 1.0.229`, `chrono 0.4.45`, `anyhow 1.0.104` — no vulnerable pins. `deny.toml` enforces `yanked="deny"` + advisories/licenses/sources, with one well-justified ignore (`RUSTSEC-2026-0192`, `ttf-parser` *unmaintained*, reached only via winit Wayland decorations, not on the Windows path).

## 3. TODO (Prioritized)

**P0:** (1) Fix association WQL interpolation, validating class as a bare identifier (`worker.rs:1557,:1569`). (2) Add Windows-gated COM-path integration tests (bind + drain + cap/deadline/cancel).
**P1:** (3) Zeroize in-memory credentials (`remote.rs:47-51,:80-114`). (4) Stop plaintext method args in the audit log, or redact by param type (`requests.rs:281-294`). (5) Document core `invoke_method` as privileged/unguarded; consider a core policy hook. (6) Verify or mark-beta the alt-cred remote transport (`remote.rs:8-19`). (7) Replace substring `is_dangerous_method` with qualifier-driven detection (`util.rs:9-29`).
**P2:** (8) Decompose `worker.rs`. (9) Audit GUI panic sites. (10) Gate `Delegate` behind explicit acknowledgement.

## 4. Bugfix

**Bug #1 — WQL identifier injection in association queries (P0).** `worker.rs:1557,:1569`. Root cause: caller-supplied class interpolated into WQL with no escaping; `}` breaks out of `{…}`. Fix: reject non-identifiers before the loop, reusing the check already in `single_class_from_wql` (`worker.rs:1676`):
```rust
fn is_bare_class(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}
// in class_associations() before the loop:
if !is_bare_class(class) { anyhow::bail!("invalid class name: {class:?}"); }
```
**Bug #2 — audit log leaks method arguments in cleartext (P1).** `requests.rs:281-294` writes `args=[{name}={value}]`; secrets passed as params hit `audit.log`. Fix: redact values for string/sensitive params, or log names + hashes.

**Bug/Hardening #3 — credentials not zeroized (P1).** `remote.rs:47-51,:88-114`. Fix: `zeroize::Zeroizing`/`SecretVec` + `Drop` wiping the `PinnedAuth` buffers.

**Bug/Hardening #4 — `is_dangerous_method` mis-classifies (P1, low).** `util.rs:9-29` substring match; advisory-only so low severity.

**Latent (note):** `Request::Query` with `timeout:None` + `max_rows:None` relies solely on user-cancel to escape a provider streaming `WBEM_S_TIMEDOUT` (`enumerate.rs:262-300`) — documented, not a hang.

## Maturity verdict

**HIGH for a 0.9.** Unusually disciplined systems code — sound COM/FFI (SID alignment+validation, RAII everywhere, one-door credential dispatcher), honest partial-result typing, strict CI (clippy-deny + cargo-deny + CodeQL + Dependabot), current deps. Production-quality for the **local/SSO** feature set; the **remote alternate-credential** feature should be treated as beta until verified. Bug count: 1 genuine security bug (injection-shaped) + 3 hardening defects + 1 latent note; no memory-safety/soundness bugs. Fix Bug #1 and add COM-path tests as the immediate priorities.
