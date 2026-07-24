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
    pub properties: Vec<PropertySchema>,
    pub methods: Vec<MethodSchema>,
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
    pub is_static: bool,
    pub in_params: Vec<ParamSchema>,
    pub out_params: Vec<ParamSchema>,
}

/// One method parameter.
#[derive(Debug, Clone, Default)]
pub struct ParamSchema {
    pub name: String,
    pub cim_type: String,
    pub id: i32,
    pub optional: bool,
}
