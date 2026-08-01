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

/// Render a variant as a list of strings — arrays element-wise, scalars as a
/// one-element list, empty/null as an empty list. Used for `ValueMap`/`Values`.
pub fn variant_to_string_vec(v: &Variant) -> Vec<String> {
    match v {
        Variant::Array(items) => items.iter().map(variant_to_string).collect(),
        Variant::Empty | Variant::Null => Vec::new(),
        other => vec![variant_to_string(other)],
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

/// Best-effort conversion of a WMI variant into a `u64`.
///
/// The string arm is not a nicety: WMI marshals `uint64` and `sint64` as
/// `VT_BSTR`, because a `VARIANT` predates a portable 64-bit integer type. So
/// `Win32_ProcessStartTrace.TIME_CREATED` — declared `uint64` in the MOF —
/// arrives as the *text* of a FILETIME, and a numeric-only conversion would
/// silently read every event's timestamp as zero.
pub fn variant_to_u64(v: &Variant) -> u64 {
    match v {
        Variant::UI1(n) => *n as u64,
        Variant::UI2(n) => *n as u64,
        Variant::UI4(n) => *n as u64,
        Variant::UI8(n) => *n,
        Variant::I1(n) => *n as u64,
        Variant::I2(n) => *n as u64,
        Variant::I4(n) => *n as u64,
        Variant::I8(n) => *n as u64,
        Variant::R4(n) => *n as u64,
        Variant::R8(n) => *n as u64,
        Variant::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Collect a WMI `uint8[]` property into bytes.
///
/// Used for `Win32_ProcessStartTrace.Sid`, which is a raw binary SID. A
/// missing, NULL or non-array value yields an empty vector rather than an
/// error: an event without an owner SID is a normal occurrence, not a fault.
pub fn variant_to_bytes(v: &Variant) -> Vec<u8> {
    match v {
        Variant::Array(items) => items
            .iter()
            .map(|item| match item {
                Variant::UI1(b) => *b,
                other => variant_to_u32(other) as u8,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u64_survives_the_bstr_marshalling_of_uint64() {
        // How a `uint64` really arrives from WMI.
        assert_eq!(
            variant_to_u64(&Variant::String("133997760000000000".into())),
            133_997_760_000_000_000
        );
        assert_eq!(variant_to_u64(&Variant::UI8(7)), 7);
        assert_eq!(variant_to_u64(&Variant::I4(7)), 7);
        assert_eq!(variant_to_u64(&Variant::Null), 0);
        assert_eq!(variant_to_u64(&Variant::String("not a number".into())), 0);
    }

    #[test]
    fn bytes_come_out_of_a_ui1_array() {
        assert_eq!(
            variant_to_bytes(&Variant::Array(vec![
                Variant::UI1(1),
                Variant::UI1(5),
                Variant::UI1(0)
            ])),
            vec![1u8, 5, 0]
        );
        assert!(variant_to_bytes(&Variant::Null).is_empty());
        assert!(variant_to_bytes(&Variant::String("nope".into())).is_empty());
    }

    #[test]
    fn u32_from_numbers_and_strings() {
        assert_eq!(variant_to_u32(&Variant::UI4(42)), 42);
        assert_eq!(variant_to_u32(&Variant::UI2(7)), 7);
        assert_eq!(variant_to_u32(&Variant::String("123".into())), 123);
        assert_eq!(variant_to_u32(&Variant::String("nope".into())), 0);
        assert_eq!(variant_to_u32(&Variant::Null), 0);
    }

    #[test]
    fn string_rendering() {
        assert_eq!(variant_to_string(&Variant::String("hi".into())), "hi");
        assert_eq!(variant_to_string(&Variant::Bool(true)), "true");
        assert_eq!(variant_to_string(&Variant::Empty), "");
        assert_eq!(
            variant_to_string(&Variant::Array(vec![Variant::UI4(1), Variant::UI4(2)])),
            "{1, 2}"
        );
    }

    #[test]
    fn string_vec_flattens_arrays() {
        assert_eq!(
            variant_to_string_vec(&Variant::Array(vec![
                Variant::String("a".into()),
                Variant::String("b".into())
            ])),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            variant_to_string_vec(&Variant::String("solo".into())),
            vec!["solo".to_string()]
        );
        assert!(variant_to_string_vec(&Variant::Null).is_empty());
    }
}
