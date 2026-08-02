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

### WMI marshals `uint64` as a `BSTR`

`Win32_ProcessStartTrace.TIME_CREATED` is declared `uint64` and arrives as
**text**. A variant conversion that only handles the numeric cases reads it as
0 — silently, with no error anywhere. The PID-reuse guard that compares an
event's creation time against `Win32_Process.CreationDate` therefore failed on
every single event, and the symptom was "command-line enrichment doesn't work",
which is a long way from the cause.

### `wmi::exec_notification_query` cannot carry two subscriptions on one thread

`QueryResultEnumerator::next` hardcodes `Next(WBEM_INFINITE, ..)`, so a thread
pumping two notification streams parks inside whichever one is quiet and
starves the other. Process start and stop are exactly that shape. The fix is
the same raw `IWbemServices::ExecNotificationQuery` with `lTimeout = 0` on both
— which also lets the monitor thread be joined on drop, something the
`WBEM_INFINITE` path cannot offer.

### The intrinsic fallback cannot see a process's owner

`Win32_ProcessStartTrace` carries a `Sid`. Its polled stand-in delivers a
`TargetInstance` that is a `Win32_Process`, and **`Win32_Process` has no `Sid`
property at all** — so a SID-based owner column is dead in the only mode an
unelevated operator can reach, and every row shows a blank user. `GetOwner`
fills it there, with the race that implies.

Related: in that mode `TargetInstance.CommandLine` is NULL for any process the
subscribing token does not own — all of session 0. That has to be reported as
*unreadable*, not as an empty command line. They are different facts, and a
security tool that conflates them is lying by omission.

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

### A wrong-credential bug does not look like an error, and there were seven

The two known ones were `list_connections` and the `__EventConsumer` guard. The
way to count the rest is to make the wrong path *fail*: put the worker in
alternate-credential mode with credentials that cannot connect, fire every
request shape at it, and see which ones still answer. Anything that answers did
so over a connection built as the current user.

Measured on the pre-Phase-5 tree (with its revert-to-local disabled, so the
worker stays on the target), 14 request shapes:

| still answered | with what |
|---|---|
| `ClassSchema` | 45 properties — of the local `Win32_Process` |
| `ClassMof` | 6,779 characters of local MOF |
| `ListInstances` | 309 local services |
| `BuildSearchIndex` | 1,494 local classes |
| `NetworkSnapshot` | `Ok` with **0 endpoints** |
| `ListEventSubscriptions` | `Ok` with **0 subscriptions** |
| `InvokeMethod` | **`ReturnValue=0` — it really ran, locally** |

The last row is the one that matters. A read that answers about the wrong
computer is a wrong answer; a *method invocation* that lands on the wrong
computer is an action taken somewhere nobody asked for. Nothing in the code
said so — the caller had set a host and credentials, and got a success.

`NetworkSnapshot`'s row is the shape of the original bug, visible: the function
returned `Ok` because its `Win32_Process` half succeeded over SSO while both
endpoint queries failed on the credentialed transport, and the endpoint failures
were swallowed by `if let Ok(..)`. One function, two transports, and the local
one silently won.

After routing everything through a single `bind`, all 15 shapes refuse.

The general lesson is about the *fix*, not the bugs: patching the two known call
sites would have left five. What removed the class was deleting the second door
— `wmi::WMIConnection` is the only transport that cannot carry a credential, and
it is no longer reachable from the worker at all.

### An empty security report and an unreadable one are not the same answer

`list_event_subscriptions` wrapped every query in `if let Ok(..)` and returned
`Ok(SubscriptionReport { subscriptions: vec![] })` when *nothing* could be read.
A persistence hunt that cannot reach `root\subscription` then reports the same
thing as a clean machine. This was found by the experiment above — it was the
one request out of fourteen that answered without touching WMI successfully at
all.

### `CoSetProxyBlanket`'s impersonation level reaches WMI, and gates *providers*

Measured locally through the ordinary request path, varying only the level:

| request | `Identify` | `Impersonate` | `Delegate` |
|---|---|---|---|
| connect probe (`Win32_OperatingSystem`) | `WBEM_E_ACCESS_DENIED` | ok | ok |
| `SELECT … FROM Win32_ComputerSystem` | `WBEM_E_ACCESS_DENIED` | 1 row | 1 row |
| reflect `Win32_Process` schema | **ok, 45 properties** | ok | ok |
| `StdRegProv.EnumKey` | `WBEM_E_PROVIDER_NOT_CAPABLE` | `ReturnValue=0` | `ReturnValue=0` |

Two things follow. The level is honoured on the **local/SSO** path, which
`docs/REDESIGN.md` said it could not be (that claim was true when the SSO path
went through the `wmi` crate and stale once it moved to `DirectConn`). And what
it gates is the *provider*: a class definition comes out of the repository and
needs no impersonation, so schema reflection works at `Identify` while every
provider-served call is refused. `Delegate` is indistinguishable from
`Impersonate` on one machine, as expected — there is no second hop to differ on.

### A credentialed *local* connection is refused before the blanket is set

`ConnectServer` returns `WBEM_E_LOCAL_CREDENTIALS (0x80041064)` in 2–13 ms for
`\\<this machine>\root\cimv2` with any credential. It is refused at the connect,
which means `CoSetProxyBlanket` is never reached — so the three impersonation
levels are **indistinguishable** on the alternate-credential path on a single
machine, and any claim about that path's blanket rests on the code, not on a
measurement.

### `Msft_Providers` has no `HostingModel` and no `Result`

Task 5.11 asked for four columns. Two of them do not exist on that class.
`SELECT HostingModel FROM Msft_Providers` and `SELECT Result FROM
Msft_Providers` are both rejected with **Invalid query** on Windows 11 26200,
and `Get-CimClass` confirms it: the class declares `provider`, `Namespace`,
`HostProcessIdentifier`, `HostingGroup`, `HostingSpecification`, `Locale`,
`TransactionIdentifier`, `User` and 20 `ProviderOperation_*` counters. Nothing
else.

`HostingModel` is real, one class over: `__Win32Provider` in the provider's
*own* namespace, joined by `Name`. That join produces `NetworkServiceHost`,
`LocalSystemHost`, `LocalServiceHost`, `WmiCore` and `Decoupled:NonCOM` for the
providers on this machine, and it is worth the extra query — it is what
explains why a provider sits in the host it sits in. Two providers here
(`DelegatorProvider`, in `root\Microsoft\Windows\Storage\PT` and `…\PT\Alt`)
have **no** `__Win32Provider` registration at all, so an empty string there is a
real state and not a lookup failure.

`Result` has no queryable home. The closest thing is `ResultCode` on the
`Msft_WmiProvider_*_Post` classes, which are **extrinsic events** — they exist
only while something is subscribed to provider instrumentation, and cannot be
selected as a column of a provider list. It was left out rather than added as a
field that is always empty.

`HostingSpecification` is undocumented as an enumeration, so it is passed
through as the `uint32` it is. Observed here it tracks `HostingModel`
one-for-one — 1 = `WmiCore`, 5 = `LocalSystemHost`, 10 = `Decoupled:NonCOM`,
12 = `NetworkServiceHost`, 13 = `LocalServiceHost` — but that is eight rows on
one build, which is an observation and not a mapping table.

### Two of eight providers are not hosted in a `WmiPrvSE` at all

The plan's perf filter is `WHERE Name LIKE 'WmiPrvSE%'`. Measured on this
machine, `Msft_ProviderSubSystem` and `SCM Event Provider` are hosted in PID
2788 — the WMI service itself, whose perf counter instance is `svchost#13`. The
name filter returns nothing for them, so a quarter of the provider list would
have shown blank stats with no indication why.

Filtering by the host PIDs the provider list actually names costs nothing:
`WHERE IDProcess=…` over five PIDs took 347–476 ms, and enumerating **all** 393
process instances took 380–388 ms. The perf provider materialises every counter
instance regardless of the `WHERE`; the clause saves marshalling and nothing
else.

### `PercentProcessorTime` is summed over every logical processor

`Win32_PerfFormattedData_PerfProc_Process.PercentProcessorTime` ranges
`0..=100 × logical CPUs`, not `0..=100`. On this 24-CPU machine the `_Total`
instance reads 2414 and a single busy `find` reads 102. Rendering a provider
host's 5 as "5 %" overstates it by 24×; the honest figure is 0.21 % and it needs
`Win32_ComputerSystem.NumberOfLogicalProcessors` to compute, which for a remote
target has to come from the target rather than from `available_parallelism`.

### The perf instance suffix is a reusable slot, not an identity

`WmiPrvSE#3` was PID 43468. That host exited, and `WmiPrvSE#3` came back as PID
37048 — measured across samples minutes apart, with the intervening mapping
observed stable for 14 samples over 3 minutes. So the `#n` suffix is not a key
across samples either, quite apart from being useless as a join key: the perf
class calls the four live hosts `WmiPrvSE`, `WmiPrvSE#1`, `WmiPrvSE#2`,
`WmiPrvSE#3` while `Win32_Process` calls all of them `WmiPrvSE.exe`. Joining on
the `Win32_Process` name folds three distinct processes into one row.
`IDProcess` is the only join.

### `[in, out]` parameters are real but rare

3 in `root\CIMV2` (all `*USBHub`/`*USBDevice.GetDescriptor`), 4 in
`root\Microsoft\Windows\Storage`, 4 in `root\wmi`, 0 in `root\StandardCimv2`.
Reading the in- and out-signature objects separately duplicates them with no
marker. `Win32_Process.Create` alone would never have exposed this.

### WQL does not evaluate `ORDER BY` locally. It refuses to run the query at all.

The redesign plan (§ "Design elements that are invented") called *"ORDER BY is
evaluated locally by the client"* a **true statement about WQL**, worth keeping
as a status-strip note. It is not true here. Measured on this machine through
`Get-CimInstance -Query` — the same `IWbemServices::ExecQuery` the app uses:

| query | result |
|---|---|
| `SELECT Name FROM Win32_Process` | 369 rows |
| `SELECT Name, ProcessId FROM Win32_Process ORDER BY Name` | **Invalid query** (0x80041017) |
| `SELECT Name FROM Win32_Process ORDER BY Name ASC` | **Invalid query** |
| `SELECT Name FROM Win32_Service ORDER BY Name` | **Invalid query** |
| `SELECT * FROM Win32_OperatingSystem ORDER BY Caption` | **Invalid query** |
| `SELECT Name, ProcessId\n  FROM Win32_Process` (multi-line, no clause) | 369 rows |
| `…\n ORDER BY Name` (multi-line, with clause) | **Invalid query** |

`WBEM_E_INVALID_QUERY` comes from the query *parser*, before a single object is
produced, so there is nothing for a client to sort. Confirmed a second time from
inside the app: the Query view run with an `ORDER BY` shows no rows and the
status bar carries `0x80041017`. Nothing in this codebase sorts a WQL clause
either — the result table sorts on a **column-header click**, which is a
different thing the user asks for explicitly.

So task 4.3's derivation stands (scan for `ORDER BY` outside a string literal —
that is the right trigger), but the sentence it triggers had to change. The view
now says *"ORDER BY is not valid WQL — sort by clicking a column"*, which is both
true and more actionable: the plan's wording implies the query runs and is merely
slow.

**Not measured:** whether any provider or namespace outside `root\CIMV2` accepts
the clause. Three classes were tried; the failure is in the parser, so a
provider-specific exception is unlikely but untested.

### Query wall time is not stable enough to ever be a constant

Four consecutive runs of the *same* query
(`SELECT Name, ProcessId, ThreadCount, WorkingSetSize FROM Win32_Process WHERE
WorkingSetSize > 40000000`, `root\CIMV2`) reported `elapsed_ms` of **65, 100, 54,
62** for 146–147 rows. `SELECT * FROM Win32_OperatingSystem` reported 79 ms for
1 row. Nothing about a hard-coded figure in a status strip would survive contact
with this; the mock's "412 ms" is a mock.

### A delivered event carries no `__CLASS`, and no class for its `TargetInstance`

`docs/REDESIGN.md` 4.11 has the Events view parsing an event's kind out of its
`__CLASS`. That property never reaches the GUI. `wmi::IWbemClassWrapper::
list_properties` calls `GetNames` with `WBEM_FLAG_ALWAYS | WBEM_FLAG_NONSYSTEM_
ONLY`, so every `__`-prefixed system property is filtered out before
`monitor::flatten_event` iterates — and `MonitorMsg::Event`, a flat
`Vec<(String, String)>`, is the whole of what the view receives.

Measured, on a live `__InstanceCreationEvent WITHIN 2 … Win32_Process`
subscription, the delivered key set is exactly:

```
["SECURITY_DESCRIPTOR", "TIME_CREATED", "TargetInstance.Caption",
 "TargetInstance.Name", "TargetInstance.ParentProcessId",
 "TargetInstance.ProcessId"]
```

The *target* class is missing for a second, independent reason: `flatten_event`
drills one level into the embedded object and copies six named scalars, none of
which is a class name. So neither the event's class nor its subject's class is
recoverable per event without changing `vmiscope-core`.

Both are therefore derived from the subscription's own query and stamped onto
each row as it arrives. The one discriminator that *does* survive is
`PreviousInstance`: only a modification carries it, which is enough to narrow a
subscription to the `__InstanceOperationEvent` superclass one third of the way
and no further.

Related, and visible in the same key set: `SECURITY_DESCRIPTOR` arrives as the
literal string `{}` rather than being dropped, so anything summarising an event
has to exclude it by name.

### `WITHIN` is a delivery batch interval, not just a polling hint

An intrinsic subscription does not trickle. Measured by changing only the
`WITHIN` value: at `WITHIN 2` events arrived stamped `09:12:09.831` and
`09:12:11.915` — two batches two seconds apart; at `WITHIN 9`, seven events
covering nine seconds of process churn all arrived on the single stamp
`09:25:18.185`. At `WITHIN 1` on
`Win32_PerfFormattedData_PerfProc_Process`, ~260 events land on one frame and
then nothing for a second.

Two consequences for any UI over this. A "delivery rate" is only meaningful
over a window several times the interval — instantaneously it is either zero or
enormous. And a per-row arrival animation keyed on "this row is new" fires on
every visible row at once, which is not what a mock built from a trickle
predicts.

### `__PATH` names the machine; `__RELPATH` does not

Both come back only when a query asks for the identity columns
(`Request::Query{include_system: true}`), and the difference between them is the
whole basis of a host-to-host diff. Read off this machine:

    __PATH     \\DESKTOP-6SAB9EN\root\CIMV2:Win32_Service.Name="ADPSvc"
    __RELPATH  Win32_Service.Name="ADPSvc"

So `__RELPATH` is a usable cross-host key, and `__PATH` is the opposite: it
differs between any two hosts by construction, on every row, forever. A compare
that did not ignore it would report every row as changed and be right about
nothing. It is in the Compare view's default ignore list for that reason, and
`__RELPATH` is its key fallback for the other half of the same reason.

### Two reads of the same machine, a fraction of a second apart, are not equal

`SELECT * FROM Win32_Process` twice in a row, keyed on `Handle`: 365 rows, 347
identical, **18 changed** and **2 gone** (a `docker.exe` and a `conhost.exe`
that exited between the two enumerations). The 18 moved on `PageFaults`,
`VirtualSize` and `PeakVirtualSize` only.

That is what a snapshot diff is up against, and it is why the ignore list is a
feature rather than a convenience: without one, "compare two machines" answers
"everything changed" even when the two machines are one machine. `Win32_Service`
on the same pair of reads is 309 rows and **309 identical**, with three columns
ignored (`InstallDate`, `ProcessId`, `__PATH`) — a class whose state is not a
live counter behaves exactly as it should.

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

### An un-virtualised list costs ~9 µs per row per frame — the row cap is not what saves you

Task 4.13 is written as though a `Vec::insert(0, ..)` + `truncate(500)` on every
event were the thing that degrades frame time under a few hundred events a
second. Measured, it is not, and the real cost is next door.

**The ingest.** 200 000 pushes of an event-shaped row (six `(String, String)`
pairs plus scalars), release build:

| retained | front-insert + truncate | `VecDeque` push/pop | ratio |
|---|---|---|---|
| 500 | 0.78 µs/event | 0.41 µs/event | 1.9× |
| 5 000 | 4.60 µs/event | 0.40 µs/event | 11.4× |

At the old 500-row cap and 200 ev/s that is 156 µs *per second*. The
front-insert was never the frame-time problem; it is what makes a **deeper** log
unaffordable, because its cost is linear in the cap while the ring's is not.

**The render.** Same build, same live subscription (~240–260 ev/s from
`__InstanceModificationEvent WITHIN 1 … Win32_PerfFormattedData_PerfProc_
Process`), timing the whole of `App::ui`:

| rows held | `ScrollArea::show` (all rows) | `show_rows` (virtualised) |
|---|---|---|
| < 1 000 | 2.7 ms mean | — |
| 1 000 | 12.8 ms | — |
| 2 000 | 22.9 ms | — |
| 3 000 | 32.2 ms | — |
| 4 000 | 40.5 ms | — |
| 5 000 | **46.3 ms mean, p99 54.5 ms** | **p50 0.63 ms, p99 1.34 ms** |

Linear in the retained rows at roughly 9 µs each, and three times over a 16.7 ms
budget by 5 000 rows — about 21 fps, with the drain falling behind as well.
Virtualisation is what buys the headroom; the ring is what makes keeping 5 000
rows cost nothing to maintain. Both were needed, for different reasons than the
plan gives.

### `animate_bool` cannot express "born this frame", and cannot loop

Two animations in the Events view are driven from `input().time` and a per-item
birth stamp instead, and neither could have been built on egui's animation API:

- `Ui::animate_bool` returns the **target** value on the first frame it sees a
  given `Id` (`AnimationManager` inserts the value it was asked for rather than
  the opposite one). A row created this frame therefore starts at "already
  finished" and never fades in. There is no "animate from" entry point.
- It eases once between two states and holds. A heartbeat needs a cycle, and
  nothing in `AnimationManager` restarts one, so a pulse built on it beats
  exactly once.

Both are cheap from the clock: `1 - easing::cubic_out((now - created_at) / d)`
for the one-shot, `(1 - cos(2π · t / period)) / 2` for the loop — a cosine is
its own ease-in-out, which is what the mock's keyframes describe.

### A debug build paints "Unaligned" over anything off the 1/32-point grid

`Ui::register_rect` is `#[cfg(debug_assertions)]` and, with
`DebugOptions::show_unaligned` (which defaults to `cfg!(debug_assertions)`),
draws an orange rule plus the word `Unaligned` along any `Ui` edge that is not a
multiple of `emath::GUI_ROUNDING` = 1/32 pt. The density scale in this project is
deliberately fractional (5.6 / 8.4 / 11.2), so a cursor a few `add_space` calls
deep is essentially never on that grid.

It shows up in Compare's withheld-diff states because there the banner is the
last thing on screen. The Explorer, captured from the same binary, shows none —
consistent with the marks being painted over rather than absent, though that was
not separately confirmed. Two fixes were tried and neither works: snapping the
frame's top onto the grid leaves the bottom off it (the height comes from font
metrics), and rounding the height as well merely adds a second flagged `Ui`
inside the first. Nothing of it reaches a release build — the function does not
exist there — so it is documented rather than fought.

### U+2260 and U+2212 are in both text faces; the glyph gate is a policy, not a coverage check

`check.ps1`'s I9 rules exist because `default_fonts` is off, so a codepoint
outside the two embedded text faces renders as a blank box. That reasoning does
not apply to every codepoint the allow-list omits. Reading the `cmap` tables of
`InterVariable.ttf` (2,852 mapped codepoints) and `JetBrainsMonoNL-Regular.ttf`
(1,363): **U+2260 `≠` and U+2212 `−` are present in both**, as is every glyph the
allow-list already permits.

So the plan's `=` `≠` `−` `+` sign column would have rendered; it is the
invariant that refuses it, and the invariant is a curated list rather than a
measurement of the fonts. Compare ships the ASCII `=` `!=` `-` `+` instead of
quietly widening a standing rule.

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

### A layout is inherited, so a control laid out "horizontally" can run backwards

`ui.horizontal` does not mean left to right. It inherits the parent's main
direction, and `Layout::right_to_left` is how you right-align anything — which
is how the Settings view aligns every control in its value column.

The result, found by capture and invisible in the source: **every segmented
control rendered its options in reverse.** `[(true, "On"), (false, "Off")]` drew
as `Off | On`, and Identify / Impersonate / Delegate — an ascending scale of how
much of your identity you hand a remote provider — ran down the screen from
Delegate. Six controls, all wrong, all reading as deliberate.

The fix is *not* to force `Layout::left_to_right` on the inner `Ui`. That fixes
the order and breaks the size: a forced-direction child claims the whole
remaining width, so the group's border stretches across the panel instead of
hugging its options — which the second capture showed. Reversing the *placement
order* when the inherited direction is right-to-left keeps both. The seam
between two options then has to move to the trailing edge as well, since "the
edge facing the option placed before this one" is the right edge when placement
runs backwards.

### egui has no focused-widget visual, but it does have `read_response`

`Response::widget_state` folds focus into `Active`, so a keyboard-focused
control is indistinguishable from a pressed one and, at rest, from an unfocused
one. The obvious answer is a `focus_ring(ui, &response)` helper that every
widget calls.

Measured against the actual code, that answer does not hold: **eleven interactive
controls had no ring**, all of them raw egui widgets a view reached for directly
— three `ui.checkbox`, eight `ui.selectable_label`, plus every
`CollapsingHeader`. A widget kit only sees the widgets that go through it, and
no audit keeps that list at zero for long.

`Context::read_response(id)` returns this frame's `Response` for any `Id`, and
`Memory::focused()` gives the focused one. One call at the end of the frame
paints the ring for whatever holds focus, in a foreground layer, whether or not
the widget has ever heard of the kit. Verified by forcing focus onto Network's
"external only" checkbox, which is stock `ui.checkbox`.

### Frame *interval* cannot tell you whether a UI is fast

The `--bench` harness measures two things, and only one of them is a
measurement. Release build, this machine, 240 frames a scenario with the first
60 discarded as warm-up:

| scenario | CPU/frame mean | p50 | p95 | max | interval p50 |
|---|---|---|---|---|---|
| 50,000-row result table (Query) | 0.89 ms | 0.84 | 1.24 | 1.34 | 13.32 ms |
| 1,400-class list (Explorer) | 0.58 ms | 0.56 | 0.76 | 0.86 | 13.33 ms |
| 2,000-event stream (Process) | 1.19 ms | 1.17 | 1.50 | 1.60 | 13.34 ms |

The interval column is **identical across all three** and is not about this
code at all: it is the display period, and it would read 13.3 ms for a UI ten
times slower. Anything of the form "holds 60 fps" that is derived from frame
timing on a vsynced surface is measuring the monitor. `eframe::Frame::info()`'s
`cpu_usage` — egui plus painting for the previous frame — is the number that
answers the question, and against a 16.67 ms budget the worst of the three uses
9%.

Two things the numbers also say. Virtualisation works: 50,000 rows cost less per
frame than 2,000 rows do in a view that computes a fade alpha, a lifetime and a
filter match per row. And the warm-up matters — the discarded frames include the
font atlas growing and `egui_extras` measuring its columns for the first time,
which is the cost of arriving rather than the cost of being there.

---

## Tooling traps

### `serde_json::from_str` into a `Value` cannot verify written key order

`Value`'s map is a `BTreeMap` here, so parsing re-sorts keys and collapses
duplicates. A round-trip test written to prove that an exporter preserves its
caller's column order therefore passes against the **buggy and the fixed
exporter alike** -- it is measuring the parser, not the writer.

The first cross-check test for the JSON exporter was written exactly that way
and was worthless. It was caught only by deliberately reverting the fix and
watching the test still pass. Verifying written order needs a line-oriented
read of the actual bytes.

This is the same shape as the icon-font bug earlier in this document: a test
that asserts the wrong invariant is worse than no test, because it also buys
confidence.

## Method

Two habits produced most of the above.

**Adversarial verification.** Research agents' claims were checked by separate
agents told to refute them. That caught a fabricated latency figure, a misread
HRESULT, and — most usefully — a bare negative presented as a positive: "the
trace query fails, therefore elevation is the gate" does not follow without the
control showing other queries succeed on the same connection.

**Measuring instead of asserting.** The row-cap gap, the 93% miss rate, the
binary-size arithmetic and the ligature problem were all invisible until
something was run and counted. Several claims in our own plan turned out to be
wrong, and they are struck through in `REDESIGN.md` rather than quietly edited,
because a plan is only worth anything if its claims are falsifiable.

**Rendering a frame and looking at it.** Distinct from running the tests, and it
found things no assertion would have. The reversed segmented controls, the
keyboard map's overlapping text, and an empty state that reported "every socket
has closed" over a chip row reading "297 active" were all in code that compiled,
passed clippy and passed its unit tests. `PrintWindow(hwnd, dc,
PW_RENDERFULLCONTENT)` renders a live frame even on a locked workstation, where
synthetic input cannot reach the window; reaching a specific view means
temporarily changing the startup default, capturing, restoring the file and
re-running the whole gate on the reverted tree.
