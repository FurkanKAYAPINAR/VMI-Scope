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
use wmi::{IWbemClassWrapper, Variant, WMIConnection};

use crate::schema::{ClassSchema, MethodSchema, ParamSchema, PropertySchema};
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

/// Read a signature object's parameters (name, CIM type, ID, optional).
fn read_params(sig: &IWbemClassObject) -> Vec<ParamSchema> {
    let wrapper = IWbemClassWrapper::new(sig.clone());
    let mut params = Vec::new();
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
                        _ => {}
                    }
                }
            }
        }
        params.push(p);
    }
    params.sort_by_key(|p| p.id);
    params
}

/// Reflect a class definition into a [`ClassSchema`].
pub fn read_class_schema(conn: &WMIConnection, class: &str) -> Result<ClassSchema> {
    let wrapper = conn.get_object(class)?;
    let obj: &IWbemClassObject = &wrapper.inner;
    let mut schema = ClassSchema {
        class: class.to_string(),
        ..Default::default()
    };

    schema.super_class = wrapper
        .get_property("__SuperClass")
        .ok()
        .map(|v| variant_to_string(&v))
        .filter(|s| !s.is_empty());

    // Class-level qualifiers.
    unsafe {
        if let Ok(qs) = obj.GetQualifierSet() {
            for (name, val) in read_qualifiers(&qs) {
                match name.to_lowercase().as_str() {
                    "description" => {
                        schema.description = Some(variant_to_string(&val)).filter(|s| !s.is_empty())
                    }
                    "abstract" => schema.is_abstract = qualifier_bool(&val),
                    _ => {}
                }
            }
        }
    }

    // Properties.
    let mut prop_names = wrapper.list_properties().unwrap_or_default();
    prop_names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
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
                if let Some(sig) = in_sig.as_ref() {
                    ms.in_params = read_params(sig);
                }
                if let Some(sig) = out_sig.as_ref() {
                    ms.out_params = read_params(sig);
                }
                let (_h, pcw) = wide(&mname);
                if let Ok(qs) = obj.GetMethodQualifierSet(pcw) {
                    for (qn, qv) in read_qualifiers(&qs) {
                        match qn.to_lowercase().as_str() {
                            "description" => {
                                ms.description =
                                    Some(variant_to_string(&qv)).filter(|s| !s.is_empty())
                            }
                            "static" => ms.is_static = qualifier_bool(&qv),
                            _ => {}
                        }
                    }
                }
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
