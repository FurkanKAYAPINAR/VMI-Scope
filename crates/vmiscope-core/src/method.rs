//! Dynamic WMI method execution.
//!
//! Introspection (method + parameter signatures) comes from the reflected
//! [`crate::schema::ClassSchema`]; *execution* uses the `wmi` crate's public
//! API only — `get_object` → `get_method` → `spawn_instance` → `put_property`
//! → `exec_method` — so no raw COM is needed here.
//!
//! This is the one place VMI-Scope can change system state, so the GUI gates
//! every invocation behind an explicit confirmation.

use anyhow::{bail, Result};
use wmi::{Variant, WMIConnection, WMIError};

use crate::value::variant_to_string;

/// `WBEM_E_ILLEGAL_OPERATION` — the documented "you cannot do that here" code.
const WBEM_E_ILLEGAL_OPERATION: i32 = 0x8004_101Eu32 as i32;
/// `WBEM_E_INVALID_METHOD` — what WMI *actually* returns when a static method
/// is invoked against an instance path (measured on `Win32_Process.Create`
/// against `Win32_Process.Handle="0"`; see `examples/probe.rs`).
const WBEM_E_INVALID_METHOD: i32 = 0x8004_102Eu32 as i32;

/// How a parameter can be edited in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Str,
    Sint,
    Uint,
    Real,
    Bool,
    /// object / reference / array — not scalar-editable in v1.
    Other,
}

/// Classify a CIM type name (as produced by `crate::reflect`) into a [`ParamKind`].
pub fn param_kind(cim_type: &str) -> ParamKind {
    if cim_type.ends_with("[]") {
        return ParamKind::Other;
    }
    match cim_type {
        "string" | "datetime" | "char16" => ParamKind::Str,
        "boolean" => ParamKind::Bool,
        "uint8" | "uint16" | "uint32" | "uint64" => ParamKind::Uint,
        "sint8" | "sint16" | "sint32" | "sint64" => ParamKind::Sint,
        "real32" | "real64" => ParamKind::Real,
        _ => ParamKind::Other,
    }
}

/// A concrete argument supplied for one input parameter.
#[derive(Debug, Clone)]
pub struct MethodArg {
    pub name: String,
    pub kind: ParamKind,
    pub value: String,
}

/// An instance a method can be invoked against.
#[derive(Debug, Clone)]
pub struct MethodTarget {
    pub path: String,
    pub label: String,
}

/// The result of a method invocation.
#[derive(Debug, Clone, Default)]
pub struct MethodOutcome {
    pub return_value: Option<String>,
    pub outputs: Vec<(String, String)>,
}

fn build_variant(arg: &MethodArg) -> Result<Variant> {
    let v = arg.value.trim();
    Ok(match arg.kind {
        ParamKind::Str => Variant::String(arg.value.clone()),
        ParamKind::Bool => Variant::Bool(v.eq_ignore_ascii_case("true") || v == "1"),
        ParamKind::Uint => Variant::UI8(
            v.parse()
                .map_err(|_| anyhow::anyhow!("'{v}' is not an unsigned integer"))?,
        ),
        ParamKind::Sint => Variant::I8(
            v.parse()
                .map_err(|_| anyhow::anyhow!("'{v}' is not an integer"))?,
        ),
        ParamKind::Real => Variant::R8(
            v.parse()
                .map_err(|_| anyhow::anyhow!("'{v}' is not a number"))?,
        ),
        ParamKind::Other => bail!("parameter '{}' has an unsupported type", arg.name),
    })
}

/// List instances of `class` as invocation targets (capped for responsiveness).
pub fn list_instances(conn: &WMIConnection, class: &str) -> Result<Vec<MethodTarget>> {
    let mut targets = Vec::new();
    let wql = format!("SELECT * FROM {class}");
    for (i, item) in conn.exec_query(&wql)?.enumerate() {
        if i >= 500 {
            break;
        }
        let Ok(obj) = item else { continue };
        let path = obj.path().unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        let label = ["Name", "Caption", "DeviceID", "__RELPATH"]
            .into_iter()
            .find_map(|k| {
                obj.get_property(k)
                    .ok()
                    .map(|v| variant_to_string(&v))
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| path.clone());
        targets.push(MethodTarget { path, label });
    }
    targets.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    Ok(targets)
}

/// Did an invocation fail *because it was aimed at the wrong kind of path*?
///
/// Both codes mean "this method does not exist on that object", which is how
/// WMI reports a static method reached through an instance path. Nothing ran,
/// so retrying elsewhere is safe.
fn wrong_target<T>(result: &std::result::Result<T, WMIError>) -> bool {
    matches!(
        result,
        Err(WMIError::HResultError { hres })
            if *hres == WBEM_E_INVALID_METHOD || *hres == WBEM_E_ILLEGAL_OPERATION
    )
}

/// Invoke `method` on `class` (static) or on `object_path` (instance).
///
/// `is_static` is advisory, not a gate. WMI omits the `Static` qualifier often
/// enough that a caller who trusted it would be unable to invoke genuinely
/// static methods, so an invocation with no instance falls back to the class
/// path, and an instance-path invocation that WMI rejects as static is retried
/// there too.
pub fn invoke_method(
    conn: &WMIConnection,
    class: &str,
    object_path: &str,
    method: &str,
    is_static: bool,
    args: &[MethodArg],
) -> Result<MethodOutcome> {
    let class_def = conn.get_object(class)?;
    let in_params = match class_def.get_method(method)? {
        Some(sig) => {
            let inst = sig.spawn_instance()?;
            for a in args {
                if a.value.trim().is_empty() {
                    continue; // leave unset -> provider default
                }
                inst.put_property(&a.name, build_variant(a)?)?;
            }
            Some(inst)
        }
        None => None,
    };

    // An instance path always contains a key assignment (`Class.Key="x"`);
    // anything else means the caller has no instance to offer.
    let instance = object_path.trim();
    let has_instance = instance.contains('=');
    let target = if is_static || !has_instance {
        class
    } else {
        instance
    };

    let mut result = conn.exec_method(target, method, in_params.as_ref());
    if target != class && wrong_target(&result) {
        result = conn.exec_method(class, method, in_params.as_ref());
    }
    let out = match result {
        Ok(out) => out,
        // Replaces the old up-front refusal: only complain about the missing
        // instance once WMI has confirmed the class path will not do.
        Err(e) if !has_instance && !is_static => {
            bail!("{class}.{method} needs an instance; invoking it on the class path failed: {e}")
        }
        Err(e) => return Err(e.into()),
    };

    let mut outcome = MethodOutcome::default();
    if let Some(o) = out {
        for name in o.list_properties().unwrap_or_default() {
            let value = o
                .get_property(&name)
                .ok()
                .map(|v| variant_to_string(&v))
                .unwrap_or_default();
            if name.eq_ignore_ascii_case("ReturnValue") {
                outcome.return_value = Some(value);
            } else {
                outcome.outputs.push((name, value));
            }
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_cim_types() {
        assert_eq!(param_kind("string"), ParamKind::Str);
        assert_eq!(param_kind("uint32"), ParamKind::Uint);
        assert_eq!(param_kind("uint8"), ParamKind::Uint);
        assert_eq!(param_kind("sint64"), ParamKind::Sint);
        assert_eq!(param_kind("boolean"), ParamKind::Bool);
        assert_eq!(param_kind("real64"), ParamKind::Real);
        assert_eq!(param_kind("string[]"), ParamKind::Other);
        assert_eq!(param_kind("object"), ParamKind::Other);
        assert_eq!(param_kind("ref:Win32_Foo"), ParamKind::Other);
    }

    #[test]
    fn build_variant_parses_scalars() {
        let uint = MethodArg {
            name: "n".into(),
            kind: ParamKind::Uint,
            value: "5".into(),
        };
        assert!(matches!(build_variant(&uint), Ok(Variant::UI8(5))));
        let boolean = MethodArg {
            name: "b".into(),
            kind: ParamKind::Bool,
            value: "true".into(),
        };
        assert!(matches!(build_variant(&boolean), Ok(Variant::Bool(true))));
        let bad = MethodArg {
            name: "n".into(),
            kind: ParamKind::Uint,
            value: "abc".into(),
        };
        assert!(build_variant(&bad).is_err());
    }

    #[test]
    fn only_wrong_path_errors_trigger_the_class_path_retry() {
        let hres = |h: u32| -> std::result::Result<(), WMIError> {
            Err(WMIError::HResultError { hres: h as i32 })
        };
        // The code WMI really returns for a static method on an instance path.
        assert!(wrong_target(&hres(0x8004_102E)));
        assert!(wrong_target(&hres(0x8004_101E)));
        // Access denied and "not found" must surface, not be retried away.
        assert!(!wrong_target(&hres(0x8004_1003)));
        assert!(!wrong_target(&hres(0x8004_1002)));
        assert!(!wrong_target::<()>(&Ok(())));
    }
}
