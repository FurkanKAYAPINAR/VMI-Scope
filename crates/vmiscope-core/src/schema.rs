//! Reflective class-schema types.
//!
//! A [`ClassSchema`] describes a WMI class the way a header file describes a
//! type: its properties (with CIM types and qualifiers like `Description` and
//! `ValueMap` enums) and its methods (with in/out parameters). It is produced
//! by reflecting over the raw `IWbemClassObject` — see `crate::reflect` — so it
//! works even for classes that have zero instances.

/// Everything we can learn about one class by reflection.
#[derive(Debug, Clone, Default)]
pub struct ClassSchema {
    pub class: String,
    pub super_class: Option<String>,
    pub description: Option<String>,
    pub is_abstract: bool,
    /// Every class-level qualifier as `(name, rendered value)`, sorted by name.
    ///
    /// Names keep WMI's own casing, which is *not* consistent: `root\CIMV2`
    /// returns `dynamic` and `provider` lowercase but `Association`, `UUID` and
    /// `SupportsCreate` capitalized. Compare case-insensitively.
    pub qualifiers: Vec<(String, String)>,
    /// Ancestors from `__Derivation`, nearest first — for `Win32_Process`:
    /// `["CIM_Process", "CIM_LogicalElement", "CIM_ManagedSystemElement"]`.
    /// Empty for a root class such as `StdRegProv`.
    pub derivation: Vec<String>,
    /// What kind of class this is, derived from `qualifiers` + `derivation`.
    pub kind: ClassKind,
    pub properties: Vec<PropertySchema>,
    pub methods: Vec<MethodSchema>,
}

/// A bit set describing what *kind* of class a [`ClassSchema`] is — the badges
/// and filter chips of the class list.
///
/// Hand-rolled rather than pulled in from `bitflags`: seven flags do not
/// justify a dependency. Combine with `|`, test with [`ClassKind::contains`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize)]
pub struct ClassKind(u8);

impl ClassKind {
    /// No kind at all — a plain, static (repository-backed) class.
    pub const NONE: Self = Self(0);
    /// `Dynamic` qualifier: instances are produced live by a provider.
    pub const DYNAMIC: Self = Self(1 << 0);
    /// `Association` qualifier: the class relates two other objects.
    pub const ASSOCIATION: Self = Self(1 << 1);
    /// Derives from `__Event`: an intrinsic or extrinsic event class.
    pub const EVENT: Self = Self(1 << 2);
    /// Name starts with `__`: a WMI system class.
    pub const SYSTEM: Self = Self(1 << 3);
    /// `Abstract` qualifier: a schema-only class with no instances of its own.
    pub const ABSTRACT: Self = Self(1 << 4);
    /// `Singleton` qualifier: exactly one instance, addressed as `Class=@`.
    pub const SINGLETON: Self = Self(1 << 5);
    /// Derives from `Win32_Perf`: a performance-counter class.
    pub const PERF: Self = Self(1 << 6);

    /// All flags in display order, with their short labels.
    const NAMED: [(Self, &'static str); 7] = [
        (Self::DYNAMIC, "Dynamic"),
        (Self::ASSOCIATION, "Association"),
        (Self::EVENT, "Event"),
        (Self::SYSTEM, "System"),
        (Self::ABSTRACT, "Abstract"),
        (Self::SINGLETON, "Singleton"),
        (Self::PERF, "Perf"),
    ];

    /// Classify a class from its name, class qualifiers and derivation chain.
    ///
    /// Qualifier *values* arrive already rendered by `crate::value`, so a WMI
    /// `VT_BOOL` reads as the string `"true"`.
    pub fn classify(class: &str, qualifiers: &[(String, String)], derivation: &[String]) -> Self {
        let set = |name: &str| {
            qualifiers
                .iter()
                .any(|(n, v)| n.eq_ignore_ascii_case(name) && v.eq_ignore_ascii_case("true"))
        };
        let derives_from = |ancestor: &str| derivation.iter().any(|d| d == ancestor);

        let mut kind = Self::NONE;
        if set("dynamic") {
            kind |= Self::DYNAMIC;
        }
        if set("association") {
            kind |= Self::ASSOCIATION;
        }
        if set("abstract") {
            kind |= Self::ABSTRACT;
        }
        if set("singleton") {
            kind |= Self::SINGLETON;
        }
        // Event-ness and perf-ness are not qualifiers at all: they are visible
        // only in the ancestry. `__Event` and `Win32_Perf` themselves are the
        // roots of those hierarchies, so they carry neither flag.
        if derives_from("__Event") {
            kind |= Self::EVENT;
        }
        if derives_from("Win32_Perf") {
            kind |= Self::PERF;
        }
        if class.starts_with("__") {
            kind |= Self::SYSTEM;
        }
        kind
    }

    /// The raw bits, for serialization or a compact comparison.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Is every flag in `other` set here?
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Is any flag in `other` set here? Used by the filter chips (OR-matching).
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// No flags at all.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Human-readable labels for the flags that are set, in display order.
    pub fn labels(self) -> Vec<&'static str> {
        Self::NAMED
            .iter()
            .filter(|(flag, _)| self.contains(*flag))
            .map(|(_, label)| *label)
            .collect()
    }

    /// Why counting this class's instances would be wrong, or `None` if it is
    /// a fair thing to ask.
    ///
    /// The three cases are not an optimization, they are a correctness rule.
    /// An abstract class has no instances *of its own* by definition; an
    /// association's "instances" are relationship tuples, so a count of them
    /// answers a question nobody asked in a column headed "instances"; and an
    /// `__Event`-derived class is a message shape, not a population — there is
    /// nothing for `CreateInstanceEnum` to return, ever.
    ///
    /// The order is most-informative-first, because the reason becomes a
    /// tooltip. Classes are routinely more than one of these: measured on
    /// `root\CIMV2`, **every** `__Event`-derived class also reports
    /// `abstract = TRUE`, propagated down from `__Event` itself with
    /// `PropagatesToSubclass` set — so ranking abstract first would mean the
    /// event reason never appeared at all. `CIM_Dependency` is likewise an
    /// abstract association.
    pub fn count_skip_reason(self) -> Option<SkipReason> {
        if self.contains(Self::EVENT) {
            Some(SkipReason::Event)
        } else if self.contains(Self::ASSOCIATION) {
            Some(SkipReason::Association)
        } else if self.contains(Self::ABSTRACT) {
            Some(SkipReason::Abstract)
        } else {
            None
        }
    }
}

impl std::ops::BitOr for ClassKind {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for ClassKind {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// One row of the class list: everything the list itself can render, and
/// nothing that would cost a second round trip to learn.
///
/// Deliberately *not* a trimmed [`ClassSchema`]. The class list shows ~1,400
/// rows in `root\CIMV2`, and the moment a row needs a property list it needs a
/// `GetObject` per row. Everything here is read off the class-definition object
/// the enumeration already handed over — see `crate::reflect::class_brief`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ClassBrief {
    pub name: String,
    pub kind: ClassKind,
    /// The `provider` qualifier — which provider DLL answers for this class,
    /// e.g. `CIMWin32`. `None` for a repository-backed (static) class, which
    /// is most of them.
    pub provider: Option<String>,
}

impl ClassBrief {
    /// A brief carrying only what a name alone can tell you.
    ///
    /// The fallback for a transport that hands over names and nothing else.
    /// `__`-prefixed means system; every other flag stays unset, which reads as
    /// "plain static class" and would be a lie if it were presented as final.
    pub fn from_name(name: impl Into<String>) -> Self {
        let name = name.into();
        let kind = ClassKind::classify(&name, &[], &[]);
        Self {
            name,
            kind,
            provider: None,
        }
    }
}

/// Why a class was left uncounted. See [`ClassKind::count_skip_reason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SkipReason {
    /// Abstract: a schema-only class, no instances of its own.
    Abstract,
    /// An association class: its instances are relationships, not objects.
    Association,
    /// Derived from `__Event`: a message shape, with no population to count.
    Event,
}

impl SkipReason {
    /// A short phrase for a tooltip.
    pub fn note(self) -> &'static str {
        match self {
            SkipReason::Abstract => "abstract class: no instances of its own",
            SkipReason::Association => "association class: instances are relationships",
            SkipReason::Event => "event class: not instantiated",
        }
    }
}

/// The result of asking how many instances a class has.
///
/// An enum rather than an `Option<usize>` plus a flag, because the two states
/// have to be impossible to confuse. "We counted zero" and "we did not count"
/// render differently, and a `0` standing in for the second is the kind of
/// quiet lie a tool like this exists to avoid.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Tally {
    /// A real enumeration happened. `completion` says whether the number is
    /// final: anything but [`crate::enumerate::Completion::Complete`] makes
    /// `instances` a lower bound, not a total.
    Counted {
        instances: usize,
        completion: crate::enumerate::Completion,
    },
    /// Deliberately not counted — see [`SkipReason`].
    Skipped(SkipReason),
}

impl Tally {
    /// The exact instance count, or `None` when there isn't one — because the
    /// class was skipped, or because the enumeration was cut short.
    pub fn exact(&self) -> Option<usize> {
        match self {
            Tally::Counted {
                instances,
                completion,
            } if completion.is_complete() => Some(*instances),
            _ => None,
        }
    }

    /// The badge text for the class list.
    ///
    /// An em dash means "not counted"; a trailing `+` means "at least this
    /// many" — the enumeration hit its budget, was cancelled, or was capped.
    /// Neither is ever rendered as a bare number, because a bare number in this
    /// column is a promise that it is the whole population.
    pub fn badge(&self) -> String {
        match self {
            Tally::Skipped(_) => "\u{2014}".to_string(),
            Tally::Counted {
                instances,
                completion,
            } => {
                if completion.is_complete() {
                    instances.to_string()
                } else {
                    format!("{instances}+")
                }
            }
        }
    }

    /// Why this tally is partial or absent, or `None` when it is an exact total.
    pub fn note(&self) -> Option<String> {
        match self {
            Tally::Skipped(reason) => Some(reason.note().to_string()),
            Tally::Counted { completion, .. } => completion.note(),
        }
    }
}

/// Class and namespace counts for one node of the namespace tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct NamespaceStats {
    pub namespace: String,
    /// Was the rollup asked for? When false, `total_classes == classes` and
    /// `namespaces == 1`.
    pub recursive: bool,
    /// Classes defined in this namespace itself.
    pub classes: usize,
    /// Direct child namespaces (`SELECT Name FROM __NAMESPACE`).
    pub children: usize,
    /// How many namespaces the rollup actually reached, this one included.
    pub namespaces: usize,
    /// Classes across every namespace the rollup reached.
    pub total_classes: usize,
    /// Namespaces that could not be bound or enumerated — almost always
    /// access denied. Counted rather than swallowed: a rollup that quietly
    /// omits `root\SECURITY` is a wrong number presented as a right one.
    pub unreadable: usize,
    /// Why the walk stopped.
    pub completion: crate::enumerate::Completion,
}

/// One relationship a class participates in.
///
/// Produced from `REFERENCES OF {Class} WHERE SchemaOnly` (which names the
/// association classes) cross-checked against `ASSOCIATORS OF {Class} WHERE
/// SchemaOnly` (which names the classes at the far end).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct AssocInfo {
    /// The association class itself, e.g. `Win32_SessionProcess`. Empty when
    /// only `ASSOCIATORS OF` reported the relationship.
    pub assoc_class: String,
    /// The reference property on `assoc_class` that points back at *our*
    /// class, e.g. `Dependent`. This is WMI's notion of a role.
    pub role: String,
    /// The class at the other end, e.g. `Win32_LogonSession`.
    pub target_class: String,
    /// Anything that qualifies the row: inheritance, self-reference, or which
    /// query it came from. Empty when the row is a plain direct relationship.
    pub note: String,
}

/// A single property and its qualifiers.
#[derive(Debug, Clone, Default)]
pub struct PropertySchema {
    pub name: String,
    /// CIM type name, e.g. `uint32`, `string[]`, `ref:Win32_Foo`.
    pub cim_type: String,
    pub is_key: bool,
    pub is_read: bool,
    pub is_write: bool,
    pub description: Option<String>,
    pub units: Option<String>,
    /// `(code, label)` pairs from `ValueMap`/`Values`, paired positionally.
    pub value_map: Vec<(String, String)>,
}

/// A method signature.
#[derive(Debug, Clone, Default)]
pub struct MethodSchema {
    pub name: String,
    pub description: Option<String>,
    /// Can this method be invoked against the *class* path, with no instance?
    ///
    /// Wider than the `Static` qualifier, which WMI omits far more often than
    /// it should: a class with no `Key` property has no instances to address,
    /// and a `Singleton` accepts the class path too. See `crate::reflect`.
    pub is_static: bool,
    /// The `Static` qualifier exactly as WMI declared it — `is_static` without
    /// the class-level inference.
    pub declared_static: bool,
    /// Parameters the caller supplies. `[in, out]` parameters live here, marked
    /// `in/out`, rather than being repeated in `out_params`.
    pub in_params: Vec<ParamSchema>,
    /// Parameters the provider fills in, including `ReturnValue`.
    pub out_params: Vec<ParamSchema>,
}

/// One method parameter.
#[derive(Debug, Clone, Default)]
pub struct ParamSchema {
    pub name: String,
    pub cim_type: String,
    pub id: i32,
    pub optional: bool,
    /// `In` qualifier — the caller supplies this value.
    pub is_in: bool,
    /// `Out` qualifier — the provider writes this value back.
    pub is_out: bool,
}

impl ParamSchema {
    /// `in`, `out` or `in/out` — the direction badge for the UI.
    pub fn direction(&self) -> &'static str {
        match (self.is_in, self.is_out) {
            (true, true) => "in/out",
            (true, false) => "in",
            (false, true) => "out",
            (false, false) => "",
        }
    }
}

/// A searchable index of a namespace: class names, and each class's property
/// (and optionally method) names. Built on demand for the global search box.
#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    pub namespace: String,
    pub classes: Vec<String>,
    pub properties: std::collections::HashMap<String, Vec<String>>,
    pub methods: std::collections::HashMap<String, Vec<String>>,
    pub has_methods: bool,
}

/// One global-search hit.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub class: String,
    /// `None` for a class hit; `Some(name)` for a property/method hit.
    pub member: Option<String>,
    pub is_method: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a qualifier list the way `reflect` hands it over.
    fn quals(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    fn chain(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// Six classifications, every input copied verbatim from a live
    /// `root\CIMV2` reflection run (see `examples/probe.rs`). Note WMI's
    /// inconsistent casing: `dynamic` and `provider` come back lowercase,
    /// `Association` and `Singleton` capitalized, `Abstract` either way.
    #[test]
    fn classify_known_classes() {
        // Win32_Process: dynamic instance provider, nothing else.
        assert_eq!(
            ClassKind::classify(
                "Win32_Process",
                &quals(&[
                    ("CreateBy", "Create"),
                    ("DeleteBy", "DeleteInstance"),
                    ("dynamic", "true"),
                    ("Locale", "1033"),
                    ("provider", "CIMWin32"),
                    ("SupportsCreate", "true"),
                    ("SupportsDelete", "true"),
                    ("UUID", "{8502C4DC-5FBB-11D2-AAC1-006008C78BC7}"),
                ]),
                &chain(&[
                    "CIM_Process",
                    "CIM_LogicalElement",
                    "CIM_ManagedSystemElement"
                ]),
            ),
            ClassKind::DYNAMIC
        );

        // Win32_LogicalDiskToPartition: a dynamic association.
        assert_eq!(
            ClassKind::classify(
                "Win32_LogicalDiskToPartition",
                &quals(&[
                    ("Association", "true"),
                    ("dynamic", "true"),
                    ("Locale", "1033"),
                    ("provider", "CIMWin32"),
                ]),
                &chain(&[
                    "CIM_LogicalDiskBasedOnPartition",
                    "CIM_BasedOn",
                    "CIM_Dependency"
                ]),
            ),
            ClassKind::DYNAMIC | ClassKind::ASSOCIATION
        );

        // __InstanceCreationEvent: system + abstract, and an event only because
        // `__Event` is in its ancestry — there is no `Event` qualifier.
        assert_eq!(
            ClassKind::classify(
                "__InstanceCreationEvent",
                &quals(&[("abstract", "true")]),
                &chain(&[
                    "__InstanceOperationEvent",
                    "__Event",
                    "__IndicationRelated",
                    "__SystemClass"
                ]),
            ),
            ClassKind::SYSTEM | ClassKind::ABSTRACT | ClassKind::EVENT
        );

        // CIM_Process: abstract schema class, no provider behind it.
        assert_eq!(
            ClassKind::classify(
                "CIM_Process",
                &quals(&[
                    ("Abstract", "true"),
                    ("Locale", "1033"),
                    ("UUID", "{8502C566-5FBB-11D2-AAC1-006008C78BC7}"),
                ]),
                &chain(&["CIM_LogicalElement", "CIM_ManagedSystemElement"]),
            ),
            ClassKind::ABSTRACT
        );

        // Win32_PerfFormattedData_PerfProc_Process: perf-ness is ancestry-only.
        assert_eq!(
            ClassKind::classify(
                "Win32_PerfFormattedData_PerfProc_Process",
                &quals(&[
                    ("Cooked", "true"),
                    ("dynamic", "true"),
                    ("HiPerf", "true"),
                    ("provider", "WmiPerfInst"),
                ]),
                &chain(&[
                    "Win32_PerfFormattedData",
                    "Win32_Perf",
                    "CIM_StatisticalInformation"
                ]),
            ),
            ClassKind::DYNAMIC | ClassKind::PERF
        );

        // Win32_WMISetting: the singleton in root\CIMV2.
        assert_eq!(
            ClassKind::classify(
                "Win32_WMISetting",
                &quals(&[
                    ("dynamic", "true"),
                    ("Locale", "1033"),
                    ("provider", "WBEMCORE"),
                    ("Singleton", "true"),
                ]),
                &chain(&["CIM_Setting"]),
            ),
            ClassKind::DYNAMIC | ClassKind::SINGLETON
        );
    }

    #[test]
    fn kind_bit_operations() {
        let k = ClassKind::DYNAMIC | ClassKind::PERF;
        assert!(k.contains(ClassKind::DYNAMIC));
        assert!(k.contains(ClassKind::DYNAMIC | ClassKind::PERF));
        assert!(!k.contains(ClassKind::EVENT));
        assert!(k.intersects(ClassKind::EVENT | ClassKind::PERF));
        assert!(!k.intersects(ClassKind::EVENT | ClassKind::SYSTEM));
        assert!(ClassKind::NONE.is_empty());
        assert!(!k.is_empty());
        assert_eq!(k.labels(), vec!["Dynamic", "Perf"]);
        assert!(ClassKind::default().labels().is_empty());
    }

    /// A qualifier that is present but `false` must not set its flag.
    #[test]
    fn false_qualifiers_do_not_classify() {
        let k = ClassKind::classify(
            "Win32_Fake",
            &quals(&[("Abstract", "false"), ("Singleton", "FALSE")]),
            &[],
        );
        assert_eq!(k, ClassKind::NONE);
    }

    /// The skip-list, stated against the six classes it has to get right.
    #[test]
    fn the_skip_list_picks_the_most_informative_reason() {
        use SkipReason::*;
        let k = |flags: ClassKind| flags.count_skip_reason();

        // Countable: a plain dynamic class, and a singleton.
        assert_eq!(k(ClassKind::DYNAMIC), None);
        assert_eq!(k(ClassKind::DYNAMIC | ClassKind::SINGLETON), None);
        // System-ness alone is not a reason -- `__Win32Provider` has instances
        // and counting them is exactly what the Providers view does.
        assert_eq!(k(ClassKind::SYSTEM | ClassKind::DYNAMIC), None);
        // Perf classes are dynamic and very much countable.
        assert_eq!(k(ClassKind::DYNAMIC | ClassKind::PERF), None);

        // CIM_Process.
        assert_eq!(k(ClassKind::ABSTRACT), Some(Abstract));
        // Win32_SessionProcess.
        assert_eq!(
            k(ClassKind::DYNAMIC | ClassKind::ASSOCIATION),
            Some(Association)
        );
        // CIM_Dependency: an abstract association reads as an association.
        assert_eq!(
            k(ClassKind::ABSTRACT | ClassKind::ASSOCIATION),
            Some(Association)
        );
        // Win32_ProcessStartTrace and __InstanceCreationEvent: every event
        // class in root\CIMV2 inherits `abstract`, so event must outrank it or
        // the reason would never be seen.
        assert_eq!(k(ClassKind::EVENT | ClassKind::ABSTRACT), Some(Event));
        assert_eq!(
            k(ClassKind::EVENT | ClassKind::ABSTRACT | ClassKind::SYSTEM),
            Some(Event)
        );
    }

    #[test]
    fn a_skipped_tally_is_an_em_dash_and_never_a_zero() {
        let skipped = Tally::Skipped(SkipReason::Abstract);
        assert_eq!(skipped.badge(), "—");
        assert_eq!(skipped.exact(), None);
        assert!(skipped.note().is_some());

        let none_found = Tally::Counted {
            instances: 0,
            completion: crate::enumerate::Completion::Complete,
        };
        assert_eq!(none_found.badge(), "0");
        assert_eq!(none_found.exact(), Some(0));
        assert!(none_found.note().is_none());
        // The two must never render the same: one is a fact, the other is the
        // absence of one.
        assert_ne!(skipped.badge(), none_found.badge());
    }

    /// A partial count is a lower bound and has to look like one.
    #[test]
    fn a_partial_tally_is_marked_as_a_lower_bound() {
        let timed_out = Tally::Counted {
            instances: 0,
            completion: crate::enumerate::Completion::TimedOut {
                after_ms: 3007,
                rows: 0,
            },
        };
        // CIM_DataFile, measured: nothing at all in three seconds.
        assert_eq!(timed_out.badge(), "0+");
        assert_eq!(timed_out.exact(), None);
        assert!(timed_out.note().unwrap().contains("timed out"));

        let cancelled = Tally::Counted {
            instances: 91,
            completion: crate::enumerate::Completion::Cancelled,
        };
        assert_eq!(cancelled.badge(), "91+");
        assert_eq!(cancelled.exact(), None);
    }

    #[test]
    fn a_brief_from_a_name_alone_knows_only_that_much() {
        let sys = ClassBrief::from_name("__InstanceCreationEvent");
        assert_eq!(sys.kind, ClassKind::SYSTEM);
        assert!(sys.provider.is_none());
        // Nothing a name can tell you makes a class abstract or an event, and
        // the fallback must not pretend otherwise.
        assert!(!sys.kind.contains(ClassKind::EVENT));

        let ordinary = ClassBrief::from_name("Win32_Process");
        assert_eq!(ordinary.kind, ClassKind::NONE);
    }

    #[test]
    fn param_direction_labels() {
        let p = |is_in, is_out| ParamSchema {
            is_in,
            is_out,
            ..Default::default()
        };
        assert_eq!(p(true, false).direction(), "in");
        assert_eq!(p(false, true).direction(), "out");
        assert_eq!(p(true, true).direction(), "in/out");
        assert_eq!(p(false, false).direction(), "");
    }
}
