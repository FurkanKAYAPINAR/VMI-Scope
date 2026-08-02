//! Dynamic WMI method execution.
//!
//! Introspection (method + parameter signatures) comes from the reflected
//! [`crate::schema::ClassSchema`]; *execution* is the raw COM sequence
//! `GetObject` → `GetMethod` → `SpawnInstance` → `Put` → `ExecMethod`.
//!
//! It used to be the `wmi` crate's equivalent, which was shorter and wrong for
//! this crate's purpose: a `WMIConnection` cannot carry alternate credentials,
//! so an invocation configured to run on a remote host as `DOMAIN\admin` ran on
//! that host as the *current* user instead. Silently mutating state as the
//! wrong principal is the worst outcome this crate has, so execution now goes
//! through [`crate::enumerate::Bound`] like everything else.
//!
//! This is the one place VMI-Scope can change system state, so the GUI gates
//! every invocation behind an explicit confirmation.

use std::time::Duration;

use anyhow::{bail, Result};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemContext, IWbemServices, WBEM_GENERIC_FLAG_TYPE,
};
use wmi::{IWbemClassWrapper, Variant};

use crate::enumerate::{drain, Bound, CancelToken, Completion};

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
#[derive(Debug, Clone, serde::Serialize)]
pub struct MethodTarget {
    pub path: String,
    pub label: String,
}

/// The result of a method invocation.
#[derive(Debug, Clone, Default, serde::Serialize)]
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

/// List instances of `class` as invocation targets.
///
/// Bounded three ways, because "list the instances of a class" is a request a
/// user can aim at `CIM_DataFile`: `cap` rows, `deadline` wall-clock, and the
/// cancellation flag. The old version pulled the `wmi` iterator with an
/// `if i >= 500 { break }` inside, which bounds nothing — that provider yields
/// no rows at all for the first several seconds, so there was never a count to
/// break on.
pub(crate) fn list_instances(
    conn: &Bound,
    class: &str,
    cap: Option<usize>,
    deadline: Option<Duration>,
    cancel: &CancelToken,
) -> Result<(Vec<MethodTarget>, Completion)> {
    // `CreateInstanceEnum` rather than `SELECT * FROM {class}`: same objects,
    // no query parser, and a class name cannot smuggle a WQL clause in.
    let en = conn.instance_enum(class, true)?;
    let (mut targets, completion) = drain(&en, cap, deadline, cancel, |obj| {
        let path = crate::reflect::string_property(obj, "__PATH");
        let label = ["Name", "Caption", "DeviceID", "__RELPATH"]
            .into_iter()
            .map(|k| crate::reflect::string_property(obj, k))
            .find(|s| !s.is_empty())
            .unwrap_or_else(|| path.clone());
        Ok(MethodTarget { path, label })
    })?;
    // An object with no `__PATH` cannot be invoked against, so it is not a
    // target -- dropped here rather than offered as a row that always fails.
    targets.retain(|t| !t.path.is_empty());
    targets.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    Ok((targets, completion))
}

/// Did an invocation fail *because it was aimed at the wrong kind of path*?
///
/// Both codes mean "this method does not exist on that object", which is how
/// WMI reports a static method reached through an instance path. Nothing ran,
/// so retrying elsewhere is safe.
fn wrong_target<T>(result: &windows::core::Result<T>) -> bool {
    matches!(
        result,
        Err(e) if e.code().0 == WBEM_E_INVALID_METHOD || e.code().0 == WBEM_E_ILLEGAL_OPERATION
    )
}

/// `IWbemServices::ExecMethod`, with the out-parameters object kept.
///
/// Returns `Ok(None)` for a `void` method with no out parameters — WMI
/// produces no object at all in that case, and that is a success, not a
/// missing result.
fn exec_method_raw(
    svc: &IWbemServices,
    target: &str,
    method: &str,
    in_params: Option<&IWbemClassObject>,
) -> windows::core::Result<Option<IWbemClassObject>> {
    let mut out: Option<IWbemClassObject> = None;
    unsafe {
        svc.ExecMethod(
            &windows::core::BSTR::from(target),
            &windows::core::BSTR::from(method),
            WBEM_GENERIC_FLAG_TYPE(0),
            None::<&IWbemContext>,
            in_params,
            Some(&mut out),
            None,
        )?;
    }
    Ok(out)
}

/// Build the in-parameters object for `method`, or `None` when it takes none.
fn spawn_in_params(
    class_def: &IWbemClassObject,
    method: &str,
    args: &[MethodArg],
) -> Result<Option<IWbemClassObject>> {
    let name = windows::core::HSTRING::from(method);
    let mut in_sig: Option<IWbemClassObject> = None;
    unsafe {
        class_def.GetMethod(
            windows::core::PCWSTR(name.as_ptr()),
            0,
            &mut in_sig,
            std::ptr::null_mut(),
        )?;
    }
    // A method with no input parameters has no in-signature object at all.
    let Some(sig) = in_sig else { return Ok(None) };
    let inst = unsafe { sig.SpawnInstance(0)? };
    for a in args {
        if a.value.trim().is_empty() {
            continue; // leave unset -> provider default
        }
        // `wmi`'s conversion, not a hand-rolled one: it encodes the WMI
        // numeric quirks (sint8 travels as VT_I2, uint64 as a decimal
        // *string*), and getting those wrong is a silently truncated argument.
        let variant: VARIANT = build_variant(a)?
            .try_into()
            .map_err(|e| anyhow::anyhow!("parameter '{}': {e}", a.name))?;
        let pname = windows::core::HSTRING::from(a.name.as_str());
        unsafe {
            inst.Put(windows::core::PCWSTR(pname.as_ptr()), 0, &variant, 0)?;
        }
    }
    Ok(Some(inst))
}

/// Invoke `method` on `class` (static) or on `object_path` (instance).
///
/// `is_static` is advisory, not a gate. WMI omits the `Static` qualifier often
/// enough that a caller who trusted it would be unable to invoke genuinely
/// static methods, so an invocation with no instance falls back to the class
/// path, and an instance-path invocation that WMI rejects as static is retried
/// there too.
pub(crate) fn invoke_method(
    conn: &Bound,
    class: &str,
    object_path: &str,
    method: &str,
    is_static: bool,
    args: &[MethodArg],
) -> Result<MethodOutcome> {
    // `GetMethod` works on a class *definition* only, never on an instance —
    // so the signature always comes from the class even when the call will be
    // aimed at an instance path.
    let class_def = conn.get_object(class)?;
    let in_params = spawn_in_params(&class_def, method, args)?;
    let svc = conn.services();

    // An instance path always contains a key assignment (`Class.Key="x"`);
    // anything else means the caller has no instance to offer.
    let instance = object_path.trim();
    let has_instance = instance.contains('=');
    let target = if is_static || !has_instance {
        class
    } else {
        instance
    };

    let mut result = exec_method_raw(svc, target, method, in_params.as_ref());
    if target != class && wrong_target(&result) {
        result = exec_method_raw(svc, class, method, in_params.as_ref());
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
        for name in IWbemClassWrapper::new(o.clone())
            .list_properties()
            .unwrap_or_default()
        {
            let value = crate::reflect::string_property(&o, &name);
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
        let hres = |h: u32| -> windows::core::Result<()> {
            Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                h as i32,
            )))
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
