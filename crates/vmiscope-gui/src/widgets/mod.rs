//! The Nocturne widget kit.
//!
//! One implementation of each control, built from `theme::tokens`, so a view
//! never reaches for a colour, a radius or a raw pixel. The design's rules --
//! outlined buttons that are never filled, rules that fade at their ends, a 2px
//! accent focus ring, a 4% hover tint -- live here rather than being restated
//! in every view.

pub(crate) mod button;
pub(crate) mod card;
pub(crate) mod chip;
pub(crate) mod codeview;
pub(crate) mod field;
pub(crate) mod kv;
pub(crate) mod loading;
pub(crate) mod rule;
pub(crate) mod table;
