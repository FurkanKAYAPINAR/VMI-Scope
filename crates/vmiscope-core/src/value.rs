//! Conversion of raw [`wmi::Variant`] values into display-friendly strings.
//!
//! WMI properties are dynamically typed. For a *generic* explorer we cannot
//! deserialize into known Rust structs, so every value arrives as a
//! [`wmi::Variant`]. [`variant_to_string`] renders one for a table cell.

use wmi::Variant;

/// Render a WMI variant as a single-line human-readable string.
///
/// Arrays become `{a, b, c}`. Embedded objects are summarized rather than
/// recursively expanded (the detail pane handles drill-down separately).
pub fn variant_to_string(v: &Variant) -> String {
    match v {
        Variant::Empty | Variant::Null => String::new(),
        Variant::String(s) => s.clone(),
        Variant::Bool(b) => b.to_string(),
        Variant::I1(n) => n.to_string(),
        Variant::I2(n) => n.to_string(),
        Variant::I4(n) => n.to_string(),
        Variant::I8(n) => n.to_string(),
        Variant::UI1(n) => n.to_string(),
        Variant::UI2(n) => n.to_string(),
        Variant::UI4(n) => n.to_string(),
        Variant::UI8(n) => n.to_string(),
        Variant::R4(n) => n.to_string(),
        Variant::R8(n) => n.to_string(),
        Variant::Array(items) => {
            let parts: Vec<String> = items.iter().map(variant_to_string).collect();
            format!("{{{}}}", parts.join(", "))
        }
        // Object / Unknown / any future variant: fall back to a compact debug form.
        other => format!("{other:?}"),
    }
}

/// Best-effort conversion of a WMI variant into a `u32` (ports, PIDs, enum
/// codes). Non-numeric or missing values yield `0`.
pub fn variant_to_u32(v: &Variant) -> u32 {
    match v {
        Variant::UI1(n) => *n as u32,
        Variant::UI2(n) => *n as u32,
        Variant::UI4(n) => *n,
        Variant::UI8(n) => *n as u32,
        Variant::I1(n) => *n as u32,
        Variant::I2(n) => *n as u32,
        Variant::I4(n) => *n as u32,
        Variant::I8(n) => *n as u32,
        Variant::R4(n) => *n as u32,
        Variant::R8(n) => *n as u32,
        Variant::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}
