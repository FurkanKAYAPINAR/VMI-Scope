# Measured findings

Things this project had to establish by measurement rather than by reading, and
that cost real time to discover. Written down because most of them are not in
any documentation, and because several contradicted what we had assumed.

Every number here was produced on Windows 11 Pro 26200, `wmi` 0.18.4,
`windows` 0.62.2, egui/eframe 0.35.0. Where something was *not* measured, it
says so — the distinction is the point.

---

## WMI

### A row cap cannot bound a query. Some providers yield nothing at all.

The obvious way to stop `SELECT * FROM CIM_DataFile` from walking an entire
filesystem is to cap the row count. It does not work, and the reason is not
obvious. Measured, with `examples/cancel.rs`:

| query | cap | deadline | result |
|---|---|---|---|
| `SELECT * FROM Win32_Process` | 5 | — | 5 rows, 34 ms, `Truncated` |
| `SELECT * FROM Win32_Process` | — | — | 368 rows, 51 ms, `Complete` |
| `SELECT * FROM CIM_DataFile` | 200 | — | **no reply in 45 s** |
| `SELECT * FROM CIM_DataFile` | 200 | 3 s | `TimedOut { after_ms: 3008, rows: 0 }` |
| `SELECT * FROM CIM_DataFile` | — | cancelled at 1 s | `Cancelled`, **0 rows**, reply 10 ms later |

Rows three and five together give the diagnosis: after a full second,
`CIM_DataFile` has produced **zero** rows. That provider materialises its whole
result before releasing the first object, so there is never anything to count
and the cap can never fire. A cap is a *row* budget; bounding wall-clock time
needs a deadline as well. Both are now on `Request::Query`.

The cancellation path is genuinely fast — 10 ms from flag to reply — because the
enumeration pulls in batches of 64 with a **finite** 200 ms `lTimeout` and reads
its flags between batches.

### `WBEM_S_TIMEDOUT` is a success code carrying data

`IEnumWbemClassObject::Next` with a finite timeout returns three success
HRESULTs. `WBEM_S_FALSE` is the only one meaning "no more rows, ever";
`WBEM_S_NO_ERROR` means the batch filled and `WBEM_S_TIMEDOUT` means it did not
fill *yet* — but both may still carry objects. Treating a short batch as
end-of-data (the natural `returned == 0` test) silently truncates every slow
provider.

### Cancel and shutdown cannot travel through the request channel

The worker is inside an uninterruptible COM call at exactly the moment someone
wants to stop it, so a `Cancel` message queued behind that work is waiting on
the thing it exists to interrupt. Both raise an atomic flag *before* the message
is sent. Before this, closing the window during a runaway query hung the app
outright, because `Drop` joins the worker thread. Now: **603 ms**.

### `Win32_ProcessStartTrace` is gated by its provider, not by being extrinsic

Our event monitor used the intrinsic, polled
`__InstanceCreationEvent WITHIN 2`. Measured against 72 instant-exit processes
with a liveness control:

```
4 runs, 72x  Start-Process cmd.exe /c exit
run1 12 spawned -> 0 caught
run2 20 spawned -> 5 caught
run3 20 spawned -> 0 caught
run4 20 spawned -> 0 caught
AGGREGATE: 67/72 MISSED = 93%
positive control: 3x cmd.exe running ping -n 7 -> 3/3 CAUGHT
```

So a polled subscription misses the large majority — but not all — of instant-exit
processes; a catch happens only when a poll boundary lands inside the process
lifetime. "Never sees them" is too strong.

`Win32_ProcessStartTrace` / `Win32_ProcessStopTrace` are the extrinsic answer,
but on a UAC-filtered admin token every subscription path is refused with
`WBEM_E_ACCESS_DENIED (0x80041003)`. Control tests show the denial is specific:

| query, same connection, same token | result |
|---|---|
| `__InstanceModificationEvent WITHIN 2 … Win32_LocalTime` | OK |
| `__InstanceCreationEvent WITHIN 2 … Win32_Process` | OK |
| `Win32_VolumeChangeEvent` (**also** `__ExtrinsicEvent`-derived) | OK |
| `Win32_ProcessStartTrace` / `StopTrace` / `ThreadStartTrace` / `ModuleLoadTrace` | **AccessDenied** |

So the gate is **not** "extrinsic events need elevation" — `Win32_VolumeChangeEvent`
is extrinsic and subscribes fine unelevated. The gate is the specific provider:
`__EventProviderRegistration` → **"WMI Kernel Trace Event Provider"**, registered
for exactly those five classes. It is also not a `root\CIMV2` ACL, since other
notification queries succeed for the same token.

The denial surfaces from `ExecNotificationQuery` itself, not from the iterator,
so error handling belongs at the subscribe call.

**Not established:** that running elevated lifts it. No session with an elevated
token was available, so the positive case is untested, as is *which* predicate
matters — integrity level, `SeDebugPrivilege`, or `Administrators` membership.
The `wmi` crate's own doctests annotate these queries *"This query will fail when
not run as admin"*, which is corroboration, not proof.

### `Win32_ProcessStartTrace` carries the owner SID, but no command line

Properties: `ProcessID`, `ParentProcessID`, `ProcessName` (bare image name, no
path), `SessionID`, `Sid`, `TIME_CREATED`, `SECURITY_DESCRIPTOR`. `StopTrace`
adds `ExitStatus`.

The SID arriving *on the event* is better than the usual approach of calling
`GetOwner` on a `Win32_Process` instance afterwards, which races the process
exiting. A command line still needs that follow-up query, and it is genuinely
racy: the process may be gone, and PID reuse can attribute the wrong command
line. `Win32_Process.CreationDate` is `CIM_DATETIME` while `TIME_CREATED` is a
`UInt64` FILETIME — different epochs *and* representations, so the obvious
comparison silently no-ops.

### Class qualifier casing is not stable

`root\CIMV2` returns `dynamic` and `provider` lowercase, but `Association`,
`Singleton` and `UUID` capitalized, and `Abstract` **both ways** depending on the
class (`CIM_Process` → `Abstract`, `__InstanceCreationEvent` → `abstract`).
Parameter directions vary the same way: `IN`, `In`, `in`. Every comparison must
be case-insensitive.

### `__Derivation` is invisible to enumeration

It is a system property, so the usual `WBEM_FLAG_NONSYSTEM_ONLY` enumeration
never lists it. It has to be fetched **by name** — on the object already in
hand, so it costs no extra round trip. Same for `__Genus`, `__Dynasty`,
`__Property_Count`.

### A static method invoked through an instance path returns `WBEM_E_INVALID_METHOD`

`0x8004102E`, not `WBEM_E_ILLEGAL_OPERATION` as we had assumed. Measured on
`Win32_Process.Create` aimed at `Win32_Process.Handle="0"`.

This matters because the `Static` qualifier is omitted far more often than not —
`Win32_OperatingSystem` carries it on **none** of its five methods. A robust
classifier also treats "class has no `Key` property" and `Singleton = TRUE` as
static-capable. Keyless non-abstract classes really exist (`CIM_USBDevice`,
`CIM_USBHub`, `CIM_StorageVolume`) and are class-path-only, because WMI cannot
address an instance of a keyless class at all.

### `[in, out]` parameters are real but rare

3 in `root\CIMV2` (all `*USBHub`/`*USBDevice.GetDescriptor`), 4 in
`root\Microsoft\Windows\Storage`, 4 in `root\wmi`, 0 in `root\StandardCimv2`.
Reading the in- and out-signature objects separately duplicates them with no
marker. `Win32_Process.Create` alone would never have exposed this.

---

## egui 0.35

### It no longer uses `ab_glyph`

`epaint 0.35` rasterizes with **skrifa 0.42** and shapes with **harfrust 0.7**
(a HarfBuzz port). Two consequences follow, and the second one bites.

**Variable fonts work.** `FontTweak { coords: VariationCoords, .. }` builds a
real `harfrust::ShaperInstance::from_variations`, not a faux-bold. Registering
the same `&'static [u8]` twice under two names with different `wght` coords
gives two weights for zero extra bytes, since `include_bytes!` emits the blob
once.

**Ligatures now fire and cannot be turned off.** egui ≤ 0.34 did no shaping, so
they never rendered. harfrust applies HarfBuzz's default feature set (`liga`,
`calt`, `clig`, `rlig`, `kern`) and epaint exposes no OpenType feature control
at all. In a tool that renders WQL and paths, stock JetBrains Mono silently
turns `!=` into `≠` and `->` into `→`. Use the **`NL` (no-ligature)** cut, which
is also 65 KB smaller.

Phosphor is affected too: it ships every icon *name* as a ligature, so if the
icon font ever led a family instead of trailing it, the words "copy", "key",
"star" and "folder" would render as pictures.

### An icon font cannot share a family with a text font, in either order

This one cost the most, and the test written to prevent it asserted the wrong
thing and guaranteed it instead.

egui resolves a family **per character, first match wins**. The obvious
arrangement is `Proportional = [Inter, Phosphor]`, so text comes from Inter and
anything Inter lacks — the icons — falls through. Measured from the three
`cmap` tables:

```
Private Use Area glyphs
  Phosphor        1513
  Inter            745     <- answers these before Phosphor ever sees them
  JetBrains Mono     7

our 94 icons, answered first by a text font
  in Proportional: 32  (34%)
  in Monospace:     1
```

A third of the icon set rendered as unrelated letters: the download arrow came
out as `Š`, the floppy as `!`, the folder as `ſ`, the refresh arrow as `.`. It
is not obviously wrong at a glance — it reads as a font that is merely a bit
odd — which is why it survived a build, a test suite and a screenshot review.

Reversing the order does not help. Phosphor covers **26 Latin letters and the
space**, because it ships each icon's *name* as an OpenType ligature. Put it
first and it answers lowercase text, and (since egui 0.35 shapes with HarfBuzz
and cannot disable ligatures) the words "copy", "key", "star" and "folder"
render as pictures.

The only arrangement where neither font is asked for a character it should not
answer is **separate families**: the icon font gets `FontFamily::Name("icons")`
and the text families do not fall back to it. Icons are then rendered by naming
that family — for icon-plus-label, a `LayoutJob` with a section each, since one
`RichText` carries exactly one family.

The lesson is about the test, not the fonts. The original test asserted "every
text family must fall back to the icon font", which is precisely the broken
arrangement, stated as an invariant and passing. A test can only protect the
property you thought to name.

### Turning off `default_fonts` costs less than leaving it on

`epaint_default_fonts` embeds **1,414,020 bytes** into every egui binary (Hack,
NotoEmoji, Ubuntu-Light, emoji-icon-font). Shipping three real faces alongside
it is pure waste. Measured with `lto = "thin", strip = true`: font bytes pass
through at **1:1** — strip removes symbols, not `.rdata`, and thin LTO never
touches an opaque byte array. So the honest arithmetic is

```
+1,576,920  three embedded faces
-1,414,020  epaint's defaults, dropped
= +162,900  net (+159 KiB)
```

The risk is not embedding fonts; it is forgetting to drop the ones egui embeds
for you. Note that dropping them also drops the emoji fallback — after which
**21 glyphs** in this app's UI resolved in neither embedded face and would have
rendered as blank boxes. That was found by parsing both fonts' `cmap` tables,
not by looking.

### `eframe`'s default feature list names `winit/default`, which you may not

A downstream crate cannot enable a feature with a slash in it, so
`default-features = false` on `eframe` silently drops winit's own defaults —
which are the Linux windowing backends. Adding `winit` as a direct dependency
restores them through feature unification.

### `const` ramps break identity lookups

egui's `Visuals` has room for exactly one accent colour, so recovering a whole
nine-step ramp means matching the live accent back to its ramp. With the ramps
declared `const`, that lookup **silently never matched**: a `const` is inlined
at every use site, so `&STEEL` in two places can be two different addresses.
`static` fixes it. The symptom would have been every tinted chip quietly
falling back to the default accent on the other two themes — which looks like a
missing token, not like a bug.

### Assorted 0.35 API notes

- `SidePanel` and `TopBottomPanel` are gone; there is one `egui::Panel` with
  `::top/::bottom/::left/::right`, and `App::ui` takes a `&mut Ui`.
- `Rounding` is `CornerRadius`. `Context::set_style`, `Context::style()` and
  `Color32::lerp` do not exist.
- `Panel::left`/`right` default to `resizable: true`; a fixed-width rail must
  say `.resizable(false)` **and** `.show_separator_line(false)`, because the
  resize-hover branch takes precedence over the flag.
- `exact_size` is the **outer** size including the frame margin, and the default
  panel frame adds `Margin::symmetric(8, 2)` — so a 40px title bar built without
  an explicit `Frame::NONE` gets 36px of content.
- `Label::halign` does not work inside a table cell; use
  `ui.with_layout(Layout::right_to_left(..))`.
- `Visuals::clip_rect_margin` is 0.0, so anything painted outside a clipped
  cell's `max_rect` is discarded entirely — a column that needs a background
  tint must be left unclipped.
- `Style::separator_style` hard-codes 6.0 spacing and is a method, not a field,
  so `ui.separator()` cannot be themed.
- `RichText::strong()` recolours; it does not embolden. Weight has to come from
  a separately registered font family.

---

## Method

Two habits produced most of the above.

**Adversarial verification.** Research agents' claims were checked by separate
agents told to refute them. That caught a fabricated latency figure, a misread
HRESULT, and — most usefully — a bare negative presented as a positive: "the
trace query fails, therefore elevation is the gate" does not follow without the
control showing other queries succeed on the same connection.

**Measuring instead of asserting.** The row-cap gap, the 93% miss rate, the
binary-size arithmetic and the ligature problem were all invisible until
something was run and counted. Three claims in our own plan turned out to be
wrong, and they are struck through in `REDESIGN.md` rather than quietly edited,
because a plan is only worth anything if its claims are falsifiable.
