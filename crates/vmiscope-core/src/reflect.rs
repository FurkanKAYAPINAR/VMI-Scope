//! Raw-COM reflection over `IWbemClassObject` — the part the `wmi` crate's
//! public API doesn't cover: property qualifiers, CIM types, and method
//! signatures. We reach the raw object through `IWbemClassWrapper.inner`
//! (which is `pub` in wmi 0.18.4), reusing the existing connection.
//!
//! All COM handles stay on the worker thread; only the plain `ClassSchema`
//! data crosses the channel.

use anyhow::Result;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi as w;
use windows::Win32::System::Wmi::{IWbemClassObject, IWbemQualifierSet};
use wmi::{IWbemClassWrapper, Variant};

use crate::schema::{
    ClassBrief, ClassKind, ClassSchema, MethodSchema, ParamSchema, PropertySchema,
};
use crate::value::{variant_to_string, variant_to_string_vec, variant_to_u32};

/// Map a CIM type code (with the array flag) to a readable name.
fn cim_type_name(cim: i32) -> String {
    let is_array = (cim & w::CIM_FLAG_ARRAY.0) != 0;
    let base = cim & 0xff;
    let name = if base == w::CIM_SINT8.0 {
        "sint8"
    } else if base == w::CIM_UINT8.0 {
        "uint8"
    } else if base == w::CIM_SINT16.0 {
        "sint16"
    } else if base == w::CIM_UINT16.0 {
        "uint16"
    } else if base == w::CIM_SINT32.0 {
        "sint32"
    } else if base == w::CIM_UINT32.0 {
        "uint32"
    } else if base == w::CIM_SINT64.0 {
        "sint64"
    } else if base == w::CIM_UINT64.0 {
        "uint64"
    } else if base == w::CIM_REAL32.0 {
        "real32"
    } else if base == w::CIM_REAL64.0 {
        "real64"
    } else if base == w::CIM_BOOLEAN.0 {
        "boolean"
    } else if base == w::CIM_STRING.0 {
        "string"
    } else if base == w::CIM_DATETIME.0 {
        "datetime"
    } else if base == w::CIM_REFERENCE.0 {
        "ref"
    } else if base == w::CIM_CHAR16.0 {
        "char16"
    } else if base == w::CIM_OBJECT.0 {
        "object"
    } else {
        "unknown"
    };
    if is_array {
        format!("{name}[]")
    } else {
        name.to_string()
    }
}

/// Build a `PCWSTR` from a string, keeping the backing `HSTRING` alive.
fn wide(s: &str) -> (HSTRING, PCWSTR) {
    let h = HSTRING::from(s);
    let p = PCWSTR(h.as_ptr());
    (h, p)
}

/// Enumerate every qualifier of a qualifier set as `(name, value)`.
///
/// End-of-enumeration is a *success* HRESULT that yields an empty name, so we
/// terminate on the empty `BSTR`, never on `Err`. A defensive cap guards
/// against a misbehaving provider.
fn read_qualifiers(qs: &IWbemQualifierSet) -> Vec<(String, Variant)> {
    let mut out = Vec::new();
    unsafe {
        if qs.BeginEnumeration(0).is_err() {
            return out;
        }
        for _ in 0..4096 {
            let mut name = windows::core::BSTR::default();
            let mut val = VARIANT::default();
            if qs
                .Next(0, &mut name, &mut val, std::ptr::null_mut())
                .is_err()
            {
                break;
            }
            if name.is_empty() {
                break;
            }
            let v = Variant::from_variant(&val).unwrap_or(Variant::Empty);
            out.push((name.to_string(), v));
        }
        let _ = qs.EndEnumeration();
    }
    out
}

fn qualifier_bool(v: &Variant) -> bool {
    matches!(v, Variant::Bool(true))
}

/// Fold one signature object's parameters into `params`, merging by name.
///
/// `from_in_sig` says which of the two signature objects we are reading, and is
/// only a fallback: the authoritative direction is the `In`/`Out` qualifier.
/// WMI spells those with no consistent case at all — `IN`, `In` and `in` all
/// occur within `root\CIMV2` — hence the lowercased match.
fn merge_params(sig: &IWbemClassObject, from_in_sig: bool, params: &mut Vec<ParamSchema>) {
    let wrapper = IWbemClassWrapper::new(sig.clone());
    for name in wrapper.list_properties().unwrap_or_default() {
        let mut p = ParamSchema {
            name: name.clone(),
            ..Default::default()
        };
        let (_h, pcw) = wide(&name);
        unsafe {
            let mut val = VARIANT::default();
            let mut cim = 0i32;
            if sig.Get(pcw, 0, &mut val, Some(&mut cim), None).is_ok() {
                p.cim_type = cim_type_name(cim);
            }
            if let Ok(qs) = sig.GetPropertyQualifierSet(pcw) {
                for (qn, qv) in read_qualifiers(&qs) {
                    match qn.to_lowercase().as_str() {
                        "id" => p.id = variant_to_u32(&qv) as i32,
                        "optional" => p.optional = qualifier_bool(&qv),
                        "in" => p.is_in = qualifier_bool(&qv),
                        "out" => p.is_out = qualifier_bool(&qv),
                        _ => {}
                    }
                }
            }
        }
        // No usable direction qualifier: infer it from the signature object the
        // parameter was found in. Rare, but a provider is free to omit it.
        if !p.is_in && !p.is_out {
            p.is_in = from_in_sig;
            p.is_out = !from_in_sig;
        }
        // An `[in, out]` parameter is present in *both* signature objects —
        // verified on `Win32_USBHub.GetDescriptor` (`RequestLength`) and
        // `MSFT_VirtualDisk.Resize` (`Size`). The second sighting must only
        // contribute its direction bit, never a second row.
        match params
            .iter_mut()
            .find(|e| e.name.eq_ignore_ascii_case(&name))
        {
            Some(existing) => {
                existing.is_in |= p.is_in;
                existing.is_out |= p.is_out;
                existing.optional |= p.optional;
                if existing.cim_type.is_empty() {
                    existing.cim_type = p.cim_type;
                }
            }
            None => params.push(p),
        }
    }
}

/// Read both signature objects of a method into one de-duplicated parameter
/// list, ordered by the `ID` qualifier (the declared parameter order).
fn read_params(
    in_sig: Option<&IWbemClassObject>,
    out_sig: Option<&IWbemClassObject>,
) -> Vec<ParamSchema> {
    let mut params = Vec::new();
    if let Some(sig) = in_sig {
        merge_params(sig, true, &mut params);
    }
    if let Some(sig) = out_sig {
        merge_params(sig, false, &mut params);
    }
    params.sort_by_key(|p| p.id);
    params
}

/// Read one property of an object *by name*.
///
/// By name, always, because the enumeration path (`list_properties`, and
/// `object_to_map` in `crate::remote`) passes `WBEM_FLAG_NONSYSTEM_ONLY` and
/// therefore never yields a `__`-prefixed property. `__CLASS` and
/// `__DERIVATION` are invisible to enumeration and free to `Get` — the object
/// is already in this process, marshalled by value.
fn property(obj: &IWbemClassObject, name: &str) -> Option<Variant> {
    let (_h, pcw) = wide(name);
    unsafe {
        let mut val = VARIANT::default();
        obj.Get(pcw, 0, &mut val, None, None).ok()?;
        Variant::from_variant(&val).ok()
    }
}

/// Read one qualifier of a qualifier set by name.
///
/// A targeted `Get` rather than the full `BeginEnumeration`/`Next` walk of
/// [`read_qualifiers`]: a class list wants five specific qualifiers out of the
/// dozen a class carries, and enumerating all of them 1,400 times over to
/// discard most is work with nothing to show for it. WMI names are
/// case-insensitive, which matters here — `root\CIMV2` spells the same
/// qualifier `dynamic` and `Abstract` and `Association` with no rule to it.
fn qualifier(qs: &IWbemQualifierSet, name: &str) -> Option<Variant> {
    let (_h, pcw) = wide(name);
    unsafe {
        let mut val = VARIANT::default();
        qs.Get(pcw, 0, &mut val, std::ptr::null_mut()).ok()?;
        Variant::from_variant(&val).ok()
    }
}

/// The qualifiers a [`ClassBrief`] is classified from, and nothing else.
const BRIEF_QUALIFIERS: [&str; 5] = [
    "Abstract",
    "Association",
    "Dynamic",
    "Singleton",
    "provider",
];

/// Summarize a class-definition object into a [`ClassBrief`].
///
/// **Costs no round trip.** Every input — the name, the derivation chain, the
/// five qualifiers — is read off the object the enumerator already handed over,
/// and a WMI class object is custom-marshalled *by value*, so it arrives whole.
/// The plan's cost model assumed a `GetObject` per class would be needed to
/// learn a class's kind; it is not, and that is the difference between a badge
/// column that is free and one that costs 1,400 round trips.
pub fn class_brief(obj: &IWbemClassObject) -> ClassBrief {
    let name = property(obj, "__CLASS")
        .map(|v| variant_to_string(&v))
        .unwrap_or_default();
    let derivation = property(obj, "__DERIVATION")
        .map(|v| variant_to_string_vec(&v))
        .unwrap_or_default();

    let mut quals: Vec<(String, String)> = Vec::new();
    unsafe {
        if let Ok(qs) = obj.GetQualifierSet() {
            for q in BRIEF_QUALIFIERS {
                if let Some(v) = qualifier(&qs, q) {
                    quals.push((q.to_string(), variant_to_string(&v)));
                }
            }
        }
    }

    let provider = quals
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("provider"))
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty());

    ClassBrief {
        kind: ClassKind::classify(&name, &quals, &derivation),
        name,
        provider,
    }
}

/// The `__DERIVATION` chain of a class object, nearest ancestor first.
pub fn class_derivation(obj: &IWbemClassObject) -> Vec<String> {
    property(obj, "__DERIVATION")
        .map(|v| variant_to_string_vec(&v))
        .unwrap_or_default()
}

/// The `__CLASS` of an object.
pub fn class_name(obj: &IWbemClassObject) -> String {
    property(obj, "__CLASS")
        .map(|v| variant_to_string(&v))
        .unwrap_or_default()
}

/// Read a named non-system property as a string — for `__NAMESPACE.Name` and
/// friends, where the whole object is one column.
pub fn string_property(obj: &IWbemClassObject, name: &str) -> String {
    property(obj, name)
        .map(|v| variant_to_string(&v))
        .unwrap_or_default()
}

/// An association class definition reduced to what an association panel needs.
#[derive(Debug, Clone, Default)]
pub struct AssocClassDef {
    pub class: String,
    /// `(reference property, class it points at)` for every `ref:` property —
    /// the association's endpoints. Two for almost every association, but the
    /// count is not guaranteed and nothing here assumes it.
    pub endpoints: Vec<(String, String)>,
}

/// Reduce an association class definition to its name and its endpoints.
///
/// The endpoint class comes from the `CIMTYPE` qualifier (`ref:Win32_Process`),
/// not from the CIM type code: `Get` reports `CIM_REFERENCE` for every
/// reference property alike and drops the target class, which is the only part
/// worth having.
pub fn assoc_class_def(obj: &IWbemClassObject) -> AssocClassDef {
    let class = class_name(obj);
    let wrapper = IWbemClassWrapper::new(obj.clone());
    let mut endpoints = Vec::new();
    for prop in wrapper.list_properties().unwrap_or_default() {
        let (_h, pcw) = wide(&prop);
        unsafe {
            if let Ok(qs) = obj.GetPropertyQualifierSet(pcw) {
                if let Some(t) = qualifier(&qs, "CIMTYPE") {
                    let t = variant_to_string(&t);
                    if let Some(target) = t.strip_prefix("ref:") {
                        endpoints.push((prop.clone(), target.to_string()));
                    }
                }
            }
        }
    }
    AssocClassDef { class, endpoints }
}

/// Reflect a class definition into a [`ClassSchema`].
///
/// Takes the object rather than a connection: a `wmi::WMIConnection` is the one
/// transport that cannot carry alternate credentials, so a function that
/// demanded one could only ever run as the current user. The caller fetches the
/// object through whichever transport its credentials require and hands it here.
pub fn read_class_schema(obj: &IWbemClassObject, class: &str) -> Result<ClassSchema> {
    let wrapper = IWbemClassWrapper::new(obj.clone());
    let mut schema = ClassSchema {
        class: class.to_string(),
        ..Default::default()
    };

    schema.super_class = property(obj, "__SuperClass")
        .map(|v| variant_to_string(&v))
        .filter(|s| !s.is_empty());

    // The derivation chain, nearest ancestor first. It has to be fetched *by
    // name*: `list_properties()` passes `WBEM_FLAG_NONSYSTEM_ONLY`, so no
    // `__`-prefixed system property ever shows up in the enumeration. This is
    // the same object we already hold, so it costs no extra COM round trip.
    // A root class such as `StdRegProv` returns an empty `VT_ARRAY | VT_BSTR`.
    schema.derivation = property(obj, "__Derivation")
        .map(|v| variant_to_string_vec(&v))
        .unwrap_or_default();

    // Class-level qualifiers. Everything is kept — `Provider`, `UUID`,
    // `SupportsCreate`, `Singleton` and friends all drive the schema panel —
    // while `Description` and `Abstract` are additionally lifted into their own
    // fields because the rest of the crate consumes them directly.
    unsafe {
        if let Ok(qs) = obj.GetQualifierSet() {
            for (name, val) in read_qualifiers(&qs) {
                let rendered = variant_to_string(&val);
                match name.to_lowercase().as_str() {
                    "description" => {
                        schema.description = Some(rendered.clone()).filter(|s| !s.is_empty())
                    }
                    "abstract" => schema.is_abstract = qualifier_bool(&val),
                    _ => {}
                }
                schema.qualifiers.push((name, rendered));
            }
        }
    }
    schema
        .qualifiers
        .sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    schema.kind = ClassKind::classify(class, &schema.qualifiers, &schema.derivation);

    // Properties.
    let mut prop_names = wrapper.list_properties().unwrap_or_default();
    prop_names.sort_by_key(|a| a.to_lowercase());
    for pname in prop_names {
        let mut ps = PropertySchema {
            name: pname.clone(),
            ..Default::default()
        };
        let (_h, pcw) = wide(&pname);
        unsafe {
            let mut val = VARIANT::default();
            let mut cim = 0i32;
            if obj.Get(pcw, 0, &mut val, Some(&mut cim), None).is_ok() {
                ps.cim_type = cim_type_name(cim);
            }
            if let Ok(qs) = obj.GetPropertyQualifierSet(pcw) {
                let mut value_map = Vec::new();
                let mut values = Vec::new();
                for (qn, qv) in read_qualifiers(&qs) {
                    match qn.to_lowercase().as_str() {
                        "key" => ps.is_key = qualifier_bool(&qv),
                        "read" => ps.is_read = qualifier_bool(&qv),
                        "write" => ps.is_write = qualifier_bool(&qv),
                        "description" => {
                            ps.description = Some(variant_to_string(&qv)).filter(|s| !s.is_empty())
                        }
                        "units" => {
                            ps.units = Some(variant_to_string(&qv)).filter(|s| !s.is_empty())
                        }
                        "valuemap" => value_map = variant_to_string_vec(&qv),
                        "values" => values = variant_to_string_vec(&qv),
                        "cimtype" => {
                            let t = variant_to_string(&qv);
                            if t.starts_with("ref:") {
                                ps.cim_type = t;
                            }
                        }
                        _ => {}
                    }
                }
                for (i, code) in value_map.into_iter().enumerate() {
                    let label = values.get(i).cloned().unwrap_or_default();
                    ps.value_map.push((code, label));
                }
            }
        }
        schema.properties.push(ps);
    }

    // Can a method of this class be invoked against the *class* path even
    // without the `Static` qualifier? Two cases say yes:
    //  - No `Key` property. WMI cannot address an instance of such a class, so
    //    there is no instance path to offer — refusing would make the method
    //    uninvokable outright (e.g. `StdRegProv`, `Win32_SecurityDescriptorHelper`).
    //  - `Singleton`. The one instance is `Class=@`, and WMI accepts the bare
    //    class path for it (e.g. `Win32_OperatingSystem`, `Win32_WMISetting`).
    // The qualifier itself is unreliable: providers omit it constantly.
    let class_level_static =
        schema.kind.contains(ClassKind::SINGLETON) || !schema.properties.iter().any(|p| p.is_key);

    // Methods.
    unsafe {
        if obj.BeginMethodEnumeration(0).is_ok() {
            for _ in 0..4096 {
                let mut name = windows::core::BSTR::new();
                let mut in_sig: Option<IWbemClassObject> = None;
                let mut out_sig: Option<IWbemClassObject> = None;
                if obj
                    .NextMethod(0, &mut name, &mut in_sig, &mut out_sig)
                    .is_err()
                {
                    break;
                }
                if name.is_empty() {
                    break;
                }
                let mname = name.to_string();
                let mut ms = MethodSchema {
                    name: mname.clone(),
                    ..Default::default()
                };
                // One merged pass over both signatures, then split by
                // direction. An `[in, out]` parameter stays in `in_params`
                // only — that is where the caller supplies it — flagged
                // `in/out` instead of appearing in both lists unmarked.
                let params = read_params(in_sig.as_ref(), out_sig.as_ref());
                ms.in_params = params.iter().filter(|p| p.is_in).cloned().collect();
                ms.out_params = params
                    .into_iter()
                    .filter(|p| p.is_out && !p.is_in)
                    .collect();

                let (_h, pcw) = wide(&mname);
                if let Ok(qs) = obj.GetMethodQualifierSet(pcw) {
                    for (qn, qv) in read_qualifiers(&qs) {
                        match qn.to_lowercase().as_str() {
                            "description" => {
                                ms.description =
                                    Some(variant_to_string(&qv)).filter(|s| !s.is_empty())
                            }
                            "static" => ms.declared_static = qualifier_bool(&qv),
                            _ => {}
                        }
                    }
                }
                ms.is_static = ms.declared_static || class_level_static;
                schema.methods.push(ms);
            }
            let _ = obj.EndMethodEnumeration();
        }
    }
    schema
        .methods
        .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(schema)
}

/// Enumerate just the method names of a class object (for the search index).
pub fn enum_method_names(obj: &IWbemClassObject) -> Vec<String> {
    let mut names = Vec::new();
    unsafe {
        if obj.BeginMethodEnumeration(0).is_ok() {
            for _ in 0..4096 {
                let mut name = windows::core::BSTR::new();
                let mut in_sig: Option<IWbemClassObject> = None;
                let mut out_sig: Option<IWbemClassObject> = None;
                if obj
                    .NextMethod(0, &mut name, &mut in_sig, &mut out_sig)
                    .is_err()
                {
                    break;
                }
                if name.is_empty() {
                    break;
                }
                names.push(name.to_string());
            }
            let _ = obj.EndMethodEnumeration();
        }
    }
    names
}

/// Return the MOF (Managed Object Format) text of a class or instance.
///
/// The object is already in this process — WMI marshals class objects by value
/// — so `GetObjectText` is a local render, not a round trip.
pub fn object_mof(obj: &IWbemClassObject) -> Result<String> {
    let text = unsafe { obj.GetObjectText(0)? };
    Ok(text.to_string())
}
