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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
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
