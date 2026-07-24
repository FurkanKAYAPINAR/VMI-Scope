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
use wmi::{Variant, WMIConnection};

use crate::value::variant_to_string;

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

/// Invoke `method` on `class` (static) or on `object_path` (instance).
pub fn invoke_method(
    conn: &WMIConnection,
    class: &str,
    object_path: &str,
    method: &str,
    is_static: bool,
    args: &[MethodArg],
) -> Result<MethodOutcome> {
    if !is_static && !object_path.contains('=') {
        bail!("no instance selected for a non-static method");
    }

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

    let target = if is_static { class } else { object_path };
    let out = conn.exec_method(target, method, in_params.as_ref())?;

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
