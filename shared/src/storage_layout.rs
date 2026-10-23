//! Checks that new contract code can still decode the storage older code wrote.
//!
//! Every upgradeable contract commits a snapshot of its storage layout as
//! `storage-layout.json`, and a unit test compares the code's current layout with
//! it ([`check_snapshot`]). The comparison asks one question: can this code
//! decode every byte the snapshot's code could have written?
//!
//! - Each storage cell keeps its key and its kind. A `Mapping` or `Lazy` field
//!   with an automatic key gets that key from its name, so renaming it moves its
//!   data, and a mapping stores entries under its key hashed with theirs, so a
//!   `Lazy` value can't become one.
//! - The packed fields of a cell keep their number, order and encoding. This is
//!   what keeps the root storage struct from changing shape.
//! - An enum may gain variants, but its existing ones keep their index and
//!   fields.
//! - New cells are allowed, removed ones are not: their data would be orphaned,
//!   and a later field could reuse the key.
//!
//! - A field or variant may be renamed, but not moved. One that now sits where
//!   another of the same type used to would still decode, into the wrong
//!   place, so a name found at a new position is an error.
//!
//! The layout ink! derives ignores SCALE attributes such as `#[codec(index)]`
//! and `#[codec(compact)]`, so the check also compares the SCALE types of each
//! cell's contents: the contract's packed fields, and each mapping's key and
//! value. Type paths are recorded for readers but never compared.
//!
//! A compatible change still fails the test until the snapshot is updated, so
//! every layout change shows up in review:
//!
//! ```text
//! UPDATE_STORAGE_LAYOUT=1 cargo test -p <contract>
//! ```
//!
//! The update refuses an incompatible change. A contract with no deployed
//! instances can start over by deleting its snapshot.

use ink::metadata::layout::{Layout, LayoutKey, RootLayout, StructLayout};
use ink::scale_info::{
    form::PortableForm, Field, IntoPortable, PortableRegistry, Registry, TypeDef, TypeInfo,
};
use ink::storage::traits::{StorageKey, StorageLayout};
use serde_json::{json, Map, Value};

/// Set this environment variable to `1` to make [`check_snapshot`] write the
/// current layout instead of requiring it to match.
pub const UPDATE_VAR: &str = "UPDATE_STORAGE_LAYOUT";

/// Fail unless contract storage `T` can decode storage written under the
/// snapshot at `path`, and has exactly its layout. With [`UPDATE_VAR`] set,
/// write the current layout to `path` instead, if it is compatible.
pub fn check_snapshot<T>(path: &str)
where
    T: StorageLayout + StorageKey + TypeInfo + 'static,
{
    let current = canonical_layout::<T>();
    let previous = read_snapshot(path);
    if let Some(previous) = &previous {
        assert_compatible(previous, &current, path);
    }

    if std::env::var(UPDATE_VAR).as_deref() == Ok("1") {
        let json = serde_json::to_string_pretty(&current).expect("layout serializes");
        std::fs::write(path, format!("{json}\n")).expect("snapshot is writable");
        return;
    }
    let previous = previous.unwrap_or_else(|| {
        panic!("No storage layout snapshot at {path}; create it with {UPDATE_VAR}=1 cargo test")
    });
    assert!(
        previous == current,
        "The storage layout changed, compatibly. Review the change, then update {path} \
         with {UPDATE_VAR}=1 cargo test"
    );
}

/// Fail unless contract storage `T` can decode storage written under the
/// snapshot at `path`. For test builds that extend a contract's layout on
/// purpose, such as the vault built with `upgrade-test`.
pub fn assert_decodes_snapshot<T>(path: &str)
where
    T: StorageLayout + StorageKey + TypeInfo + 'static,
{
    let previous =
        read_snapshot(path).unwrap_or_else(|| panic!("No storage layout snapshot at {path}"));
    assert_compatible(&previous, &canonical_layout::<T>(), path);
}

/// The storage layout of contract storage `T` in canonical form: every cell by
/// key, with the types it encodes expanded inline. This is the layout ink!
/// writes into the contract's metadata.
pub fn canonical_layout<T>() -> Value
where
    T: StorageLayout + StorageKey + TypeInfo + 'static,
{
    let key = T::KEY;
    let layout = Layout::Root(RootLayout::new(
        LayoutKey::from(key),
        T::layout(&key),
        ink::scale_info::meta_type::<T>(),
    ));
    let mut registry = Registry::new();
    let layout = layout.into_portable(&mut registry);
    let types = PortableRegistry::from(registry);

    let mut cells = Map::new();
    walk(&layout, "", &types, &mut cells);
    json!({ "cells": cells })
}

/// Why code with layout `new` can't decode storage written with layout `old`.
/// Empty when it can.
pub fn incompatibilities(old: &Value, new: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let empty = Map::new();
    let old_cells = old["cells"].as_object().unwrap_or(&empty);
    let new_cells = new["cells"].as_object().unwrap_or(&empty);
    for (key, old_cell) in old_cells {
        let name = old_cell["name"].as_str().unwrap_or(key);
        let Some(new_cell) = new_cells.get(key) else {
            errors.push(format!(
                "cell {key} ({name}) is gone: its data would be orphaned, and a later \
                 field could reuse the key"
            ));
            continue;
        };
        if old_cell["kind"] != new_cell["kind"] {
            errors.push(format!(
                "cell {key} ({name}) changed from {} to {}, which store their data differently",
                old_cell["kind"], new_cell["kind"]
            ));
            continue;
        }
        for (field, label) in [("keyType", "key"), ("valueType", "value")] {
            match (old_cell.get(field), new_cell.get(field)) {
                (Some(old_type), Some(new_type)) => compare_types(
                    old_type,
                    new_type,
                    &format!("{name} ({label})"),
                    &mut errors,
                ),
                (None, None) => {}
                _ => errors.push(format!("{name} gained or lost its {label} type")),
            }
        }
        if let Some(old_fields) = old_cell.get("packedType").and_then(Value::as_array) {
            match new_cell.get("packedType").and_then(Value::as_array) {
                Some(new_fields) => compare_fields(old_fields, new_fields, name, &mut errors),
                None => errors.push(format!("{name} lost its packed fields")),
            }
        }
        compare_layouts(&old_cell["value"], &new_cell["value"], name, &mut errors);
    }
    errors
}

fn read_snapshot(path: &str) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|err| panic!("{path} is not JSON: {err}")))
}

fn assert_compatible(previous: &Value, current: &Value, path: &str) {
    let errors = incompatibilities(previous, current);
    assert!(
        errors.is_empty(),
        "This code can't decode storage written under {path}:\n- {}",
        errors.join("\n- ")
    );
}

/// The canonical packed layout of `layout`, registering every root it contains
/// (the contract's own cell, and each `Mapping` or `Lazy` field) in `cells`.
/// A root returns `None`, since it occupies no bytes in its parent's cell.
fn walk(
    layout: &Layout<PortableForm>,
    name: &str,
    types: &PortableRegistry,
    cells: &mut Map<String, Value>,
) -> Option<Value> {
    match layout {
        Layout::Root(root) => {
            let ty = types
                .resolve(root.ty().id)
                .expect("root type is registered");
            let name = if name.is_empty() {
                ty.path.segments.last().cloned().unwrap_or_default()
            } else {
                name.to_owned()
            };
            let kind = match ty.path.segments.last().map(String::as_str) {
                Some("Mapping") => "Mapping",
                Some("Lazy") => "Lazy",
                Some("StorageVec") => "StorageVec",
                _ => "Contract",
            };
            let mut cell = Map::new();
            cell.insert("name".into(), json!(name));
            cell.insert("kind".into(), json!(kind));
            if kind == "Contract" {
                // The SCALE types of the packed fields, with the encoding details
                // the layout leaves out.
                if let TypeDef::Composite(composite) = &ty.type_def {
                    let packed: Vec<_> = composite
                        .fields
                        .iter()
                        .filter(|field| !is_storage_cell(field.ty.id, types))
                        .cloned()
                        .collect();
                    let fields = canonical_fields(&packed, types, &mut Vec::new());
                    cell.insert("packedType".into(), json!(fields));
                }
            } else {
                // A mapping's key type decides where each of its entries is stored.
                for (param, field) in [("K", "keyType"), ("V", "valueType")] {
                    let id = ty
                        .type_params
                        .iter()
                        .find(|type_param| type_param.name == param)
                        .and_then(|type_param| type_param.ty);
                    if let Some(id) = id {
                        cell.insert(field.into(), canonical_type(id.id, types, &mut Vec::new()));
                    }
                }
            }
            let value = walk(root.layout(), &name, types, cells).unwrap_or(Value::Null);
            cell.insert("value".into(), value);
            cells.insert(
                format!("0x{:08x}", root.root_key().key()),
                Value::Object(cell),
            );
            None
        }
        Layout::Leaf(leaf) => {
            Some(json!({ "leaf": canonical_type(leaf.ty().id, types, &mut Vec::new()) }))
        }
        Layout::Struct(layout) => Some(walk_struct(layout, name, types, cells)),
        Layout::Enum(layout) => {
            let variants: Map<String, Value> = layout
                .variants()
                .iter()
                .map(|(discriminant, variant)| {
                    let value = walk_struct(variant, name, types, cells);
                    (discriminant.value().to_string(), value)
                })
                .collect();
            Some(json!({ "enum": layout.name(), "variants": variants }))
        }
        Layout::Array(layout) => Some(json!({
            "array": layout.len(),
            "layout": walk(layout.layout(), name, types, cells),
        })),
        Layout::Hash(layout) => Some(json!({ "hash": walk(layout.layout(), name, types, cells) })),
    }
}

/// Whether type `id` is an ink! storage cell (`Mapping`, `Lazy`, `StorageVec`),
/// which takes no bytes in its parent's cell.
fn is_storage_cell(id: u32, types: &PortableRegistry) -> bool {
    let ty = types.resolve(id).expect("type is registered");
    ty.path.segments.len() > 2 && ty.path.segments[..2] == ["ink_storage", "lazy"]
}

fn walk_struct(
    layout: &StructLayout<PortableForm>,
    name: &str,
    types: &PortableRegistry,
    cells: &mut Map<String, Value>,
) -> Value {
    let fields: Vec<Value> = layout
        .fields()
        .iter()
        .filter_map(|field| {
            let path = if name.is_empty() {
                field.name().to_owned()
            } else {
                format!("{name}.{}", field.name())
            };
            walk(field.layout(), &path, types, cells)
                .map(|layout| json!({ "name": field.name(), "layout": layout }))
        })
        .collect();
    json!({ "struct": layout.name(), "fields": fields })
}

/// Type `id` with its definition expanded inline. A type that contains itself
/// is cut off with a reference to its path.
fn canonical_type(id: u32, types: &PortableRegistry, visiting: &mut Vec<u32>) -> Value {
    let ty = types.resolve(id).expect("type is registered");
    let path = ty.path.segments.join("::");
    if visiting.contains(&id) {
        return json!({ "ref": path });
    }
    visiting.push(id);
    let def = match &ty.type_def {
        TypeDef::Composite(composite) => {
            json!({ "composite": canonical_fields(&composite.fields, types, visiting) })
        }
        TypeDef::Variant(variant) => json!({
            "variant": variant
                .variants
                .iter()
                .map(|variant| json!({
                    "name": variant.name,
                    "index": variant.index,
                    "fields": canonical_fields(&variant.fields, types, visiting),
                }))
                .collect::<Vec<_>>(),
        }),
        TypeDef::Sequence(sequence) => {
            json!({ "sequence": canonical_type(sequence.type_param.id, types, visiting) })
        }
        TypeDef::Array(array) => json!({
            "array": { "len": array.len, "type": canonical_type(array.type_param.id, types, visiting) },
        }),
        TypeDef::Tuple(tuple) => json!({
            "tuple": tuple
                .fields
                .iter()
                .map(|field| canonical_type(field.id, types, visiting))
                .collect::<Vec<_>>(),
        }),
        TypeDef::Primitive(primitive) => {
            json!({ "primitive": format!("{primitive:?}").to_lowercase() })
        }
        TypeDef::Compact(compact) => {
            json!({ "compact": canonical_type(compact.type_param.id, types, visiting) })
        }
        TypeDef::BitSequence(bits) => json!({
            "bitSequence": {
                "store": canonical_type(bits.bit_store_type.id, types, visiting),
                "order": canonical_type(bits.bit_order_type.id, types, visiting),
            },
        }),
    };
    visiting.pop();

    let mut canonical = Map::new();
    if !path.is_empty() {
        canonical.insert("path".into(), json!(path));
    }
    canonical.insert("def".into(), def);
    Value::Object(canonical)
}

fn canonical_fields(
    fields: &[Field<PortableForm>],
    types: &PortableRegistry,
    visiting: &mut Vec<u32>,
) -> Vec<Value> {
    fields
        .iter()
        .map(|field| json!({ "name": field.name, "type": canonical_type(field.ty.id, types, visiting) }))
        .collect()
}

fn compare_layouts(old: &Value, new: &Value, at: &str, errors: &mut Vec<String>) {
    if let Some(old_leaf) = old.get("leaf") {
        match new.get("leaf") {
            Some(new_leaf) => compare_types(old_leaf, new_leaf, at, errors),
            None => errors.push(format!("{at} is no longer a single encoded value")),
        }
    } else if let Some(old_fields) = old.get("fields").and_then(Value::as_array) {
        let Some(new_fields) = new.get("fields").and_then(Value::as_array) else {
            errors.push(format!("{at} is no longer a struct"));
            return;
        };
        if old_fields.len() != new_fields.len() {
            errors.push(format!(
                "{at} encodes {} packed fields instead of {}; packed storage must keep its shape",
                new_fields.len(),
                old_fields.len()
            ));
            return;
        }
        check_moved(
            &positions(old_fields, "name"),
            &positions(new_fields, "name"),
            at,
            errors,
        );
        for (old_field, new_field) in old_fields.iter().zip(new_fields) {
            let field = format!("{at}.{}", old_field["name"].as_str().unwrap_or("?"));
            compare_layouts(&old_field["layout"], &new_field["layout"], &field, errors);
        }
    } else if let Some(old_variants) = old.get("variants").and_then(Value::as_object) {
        let Some(new_variants) = new.get("variants").and_then(Value::as_object) else {
            errors.push(format!("{at} is no longer an enum"));
            return;
        };
        let named = |variants: &Map<String, Value>| -> Vec<(String, Option<String>)> {
            variants
                .iter()
                .map(|(discriminant, variant)| {
                    (
                        discriminant.clone(),
                        variant["struct"].as_str().map(str::to_owned),
                    )
                })
                .collect()
        };
        check_moved(&named(old_variants), &named(new_variants), at, errors);
        for (discriminant, old_variant) in old_variants {
            match new_variants.get(discriminant) {
                Some(new_variant) => compare_layouts(
                    old_variant,
                    new_variant,
                    &format!("{at}#{discriminant}"),
                    errors,
                ),
                None => errors.push(format!("{at} lost variant {discriminant}")),
            }
        }
    } else if let Some(old_len) = old.get("array") {
        if new.get("array") != Some(old_len) {
            errors.push(format!("{at} changed its array length"));
            return;
        }
        compare_layouts(&old["layout"], &new["layout"], at, errors);
    } else if let Some(old_inner) = old.get("hash") {
        match new.get("hash") {
            Some(new_inner) => compare_layouts(old_inner, new_inner, at, errors),
            None => errors.push(format!("{at} is no longer hashed storage")),
        }
    } else if old != new {
        errors.push(format!("{at} changed its storage layout"));
    }
}

/// Whether every value encoded as `old` decodes, unchanged, as `new`.
fn compare_types(old: &Value, new: &Value, at: &str, errors: &mut Vec<String>) {
    if old.get("ref").is_some() || new.get("ref").is_some() {
        if old != new {
            errors.push(format!("{at} changed type"));
        }
        return;
    }
    let (old_def, new_def) = (&old["def"], &new["def"]);
    if let Some(old_fields) = old_def.get("composite").and_then(Value::as_array) {
        match new_def.get("composite").and_then(Value::as_array) {
            Some(new_fields) => compare_fields(old_fields, new_fields, at, errors),
            None => errors.push(format!("{at} is no longer a struct")),
        }
    } else if let Some(old_variants) = old_def.get("variant").and_then(Value::as_array) {
        let Some(new_variants) = new_def.get("variant").and_then(Value::as_array) else {
            errors.push(format!("{at} is no longer an enum"));
            return;
        };
        let indexed = |variants: &[Value]| -> Vec<(String, Option<String>)> {
            variants
                .iter()
                .map(|variant| {
                    (
                        variant["index"].to_string(),
                        variant["name"].as_str().map(str::to_owned),
                    )
                })
                .collect()
        };
        check_moved(&indexed(old_variants), &indexed(new_variants), at, errors);
        for old_variant in old_variants {
            let index = &old_variant["index"];
            let name = old_variant["name"].as_str().unwrap_or("?");
            match new_variants
                .iter()
                .find(|variant| &variant["index"] == index)
            {
                Some(new_variant) => compare_fields(
                    old_variant["fields"]
                        .as_array()
                        .map_or(&[][..], Vec::as_slice),
                    new_variant["fields"]
                        .as_array()
                        .map_or(&[][..], Vec::as_slice),
                    &format!("{at}::{name}"),
                    errors,
                ),
                None => errors.push(format!("{at} lost variant {name} (index {index})")),
            }
        }
    } else if let Some(old_inner) = old_def.get("sequence") {
        match new_def.get("sequence") {
            Some(new_inner) => compare_types(old_inner, new_inner, &format!("{at}[]"), errors),
            None => errors.push(format!("{at} is no longer a sequence")),
        }
    } else if let Some(old_array) = old_def.get("array") {
        match new_def.get("array") {
            Some(new_array) if new_array["len"] == old_array["len"] => compare_types(
                &old_array["type"],
                &new_array["type"],
                &format!("{at}[]"),
                errors,
            ),
            _ => errors.push(format!("{at} is no longer an array of the same length")),
        }
    } else if let Some(old_items) = old_def.get("tuple").and_then(Value::as_array) {
        match new_def.get("tuple").and_then(Value::as_array) {
            Some(new_items) if new_items.len() == old_items.len() => {
                for (index, (old_item, new_item)) in old_items.iter().zip(new_items).enumerate() {
                    compare_types(old_item, new_item, &format!("{at}.{index}"), errors);
                }
            }
            _ => errors.push(format!("{at} is no longer a tuple of the same size")),
        }
    } else if let Some(old_inner) = old_def.get("compact") {
        match new_def.get("compact") {
            Some(new_inner) => compare_types(old_inner, new_inner, at, errors),
            None => errors.push(format!("{at} is no longer compact-encoded")),
        }
    } else if old_def != new_def {
        errors.push(format!("{at} changed type"));
    }
}

fn compare_fields(old: &[Value], new: &[Value], at: &str, errors: &mut Vec<String>) {
    if old.len() != new.len() {
        errors.push(format!(
            "{at} has {} fields instead of {}",
            new.len(),
            old.len()
        ));
        return;
    }
    check_moved(&positions(old, "name"), &positions(new, "name"), at, errors);
    for (index, (old_field, new_field)) in old.iter().zip(new).enumerate() {
        let field = old_field["name"]
            .as_str()
            .map_or_else(|| index.to_string(), str::to_owned);
        compare_types(
            &old_field["type"],
            &new_field["type"],
            &format!("{at}.{field}"),
            errors,
        );
    }
}

/// Each item's position and its `name` field, if it has one.
fn positions(items: &[Value], name: &str) -> Vec<(String, Option<String>)> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| (index.to_string(), item[name].as_str().map(str::to_owned)))
        .collect()
}

/// Report names that sit at a different position in `new` than in `old`: the
/// bytes still decode, but into the wrong field or variant. A name that is gone
/// from `new` was renamed, which is safe.
fn check_moved(
    old: &[(String, Option<String>)],
    new: &[(String, Option<String>)],
    at: &str,
    errors: &mut Vec<String>,
) {
    for (old_position, name) in old {
        let Some(name) = name else { continue };
        let moved_to = new
            .iter()
            .find(|(_, new_name)| new_name.as_deref() == Some(name.as_str()))
            .map(|(position, _)| position)
            .filter(|position| *position != old_position);
        if let Some(new_position) = moved_to {
            errors.push(format!(
                "{at}: {name} moved from position {old_position} to {new_position}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ink::primitives::AccountId;
    use ink::storage::{traits::ManualKey, Lazy, Mapping};

    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    enum Status {
        Open,
        Closed,
    }

    #[allow(dead_code)]
    #[ink::storage_item]
    struct Sample {
        owner: AccountId,
        total: u64,
        status: Status,
        balances: Mapping<AccountId, u128, ManualKey<0x1000>>,
        note: Lazy<Option<u32>, ManualKey<0x2000>>,
    }

    // Variations of `Sample` that encode differently while ink!'s layout stays
    // the same, or that change a cell's kind.
    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    enum ReindexedStatus {
        Open,
        #[codec(index = 5)]
        Closed,
    }

    #[allow(dead_code)]
    #[ink::storage_item]
    struct ReindexedSample {
        owner: AccountId,
        total: u64,
        status: ReindexedStatus,
        balances: Mapping<AccountId, u128, ManualKey<0x1000>>,
        note: Lazy<Option<u32>, ManualKey<0x2000>>,
    }

    #[allow(dead_code)]
    #[ink::storage_item]
    struct MappedNoteSample {
        owner: AccountId,
        total: u64,
        status: Status,
        balances: Mapping<AccountId, u128, ManualKey<0x1000>>,
        note: Mapping<u32, Option<u32>, ManualKey<0x2000>>,
    }

    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    struct Amounts {
        total: u64,
    }

    #[allow(dead_code)]
    #[derive(Clone, Copy)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    struct CompactAmounts {
        #[codec(compact)]
        total: u64,
    }

    #[allow(dead_code)]
    #[ink::storage_item]
    struct AmountsSample {
        amounts: Amounts,
    }

    #[allow(dead_code)]
    #[ink::storage_item]
    struct CompactAmountsSample {
        amounts: CompactAmounts,
    }

    fn sample() -> Value {
        canonical_layout::<Sample>()
    }

    fn root(layout: &mut Value) -> &mut Value {
        let key = layout["cells"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, cell)| cell["name"] == "Sample")
            .map(|(key, _)| key.clone())
            .unwrap();
        &mut layout["cells"][key]["value"]
    }

    #[test]
    fn records_each_cell_by_key_with_its_packed_fields() {
        let mut layout = sample();
        let cells = layout["cells"].as_object().unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(cells["0x00001000"]["name"], "Sample.balances");
        assert!(cells["0x00001000"].get("keyType").is_some());
        assert_eq!(cells["0x00002000"]["name"], "Sample.note");

        // The mapping and the lazy field occupy no bytes in the root cell.
        let fields = root(&mut layout)["fields"].as_array().unwrap();
        let names: Vec<_> = fields
            .iter()
            .map(|field| field["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["owner", "total", "status"]);
    }

    #[test]
    fn accepts_its_own_layout_and_renamed_fields() {
        let old = sample();
        assert!(incompatibilities(&old, &old).is_empty());

        let mut renamed = old.clone();
        root(&mut renamed)["fields"][1]["name"] = json!("amount");
        assert!(incompatibilities(&old, &renamed).is_empty());
    }

    #[test]
    fn refuses_a_changed_or_added_packed_field() {
        let old = sample();

        let mut retyped = old.clone();
        root(&mut retyped)["fields"][1]["layout"]["leaf"]["def"] = json!({ "primitive": "u32" });
        assert_eq!(incompatibilities(&old, &retyped).len(), 1);

        let mut extended = old.clone();
        let total = root(&mut extended)["fields"][1].clone();
        root(&mut extended)["fields"]
            .as_array_mut()
            .unwrap()
            .push(total);
        assert_eq!(incompatibilities(&old, &extended).len(), 1);
    }

    #[test]
    fn refuses_fields_or_variants_that_swap_places() {
        let old = sample();

        // `owner` and `total` differ in type, so swap the names of two fields of
        // the same type instead: the bytes decode, into the wrong fields.
        let mut swapped = old.clone();
        let fields = root(&mut swapped)["fields"].as_array_mut().unwrap();
        fields[0]["name"] = json!("total");
        fields[1]["name"] = json!("owner");
        assert!(!incompatibilities(&old, &swapped).is_empty());

        let mut variants = old.clone();
        let status = &mut root(&mut variants)["fields"][2]["layout"]["variants"];
        status["0"]["struct"] = json!("Closed");
        status["1"]["struct"] = json!("Open");
        assert_eq!(incompatibilities(&old, &variants).len(), 2);
    }

    #[test]
    fn accepts_new_enum_variants_but_not_lost_ones() {
        let old = sample();
        let mut added = old.clone();
        let variants = root(&mut added)["fields"][2]["layout"]["variants"]
            .as_object_mut()
            .unwrap();
        let mut paused = variants["1"].clone();
        paused["struct"] = json!("Paused");
        variants.insert("2".into(), paused);
        assert!(incompatibilities(&old, &added).is_empty());
        // The old code can't read the new variant, but that is the other direction.
        assert_eq!(incompatibilities(&added, &old).len(), 1);
    }

    #[test]
    fn refuses_encoding_changes_the_layout_does_not_show() {
        // A variant's SCALE index moves, and a nested field becomes compact.
        assert!(!incompatibilities(&sample(), &canonical_layout::<ReindexedSample>()).is_empty());
        assert!(!incompatibilities(
            &canonical_layout::<AmountsSample>(),
            &canonical_layout::<CompactAmountsSample>(),
        )
        .is_empty());
    }

    #[test]
    fn refuses_a_lazy_value_that_becomes_a_mapping() {
        let errors = incompatibilities(&sample(), &canonical_layout::<MappedNoteSample>());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("changed from \"Lazy\" to \"Mapping\""));
    }

    #[test]
    fn accepts_new_cells_but_not_removed_ones() {
        let old = sample();
        let mut added = old.clone();
        let note = added["cells"]["0x00002000"].clone();
        added["cells"]["0x00003000"] = note;
        assert!(incompatibilities(&old, &added).is_empty());

        let mut removed = old.clone();
        removed["cells"]
            .as_object_mut()
            .unwrap()
            .remove("0x00002000");
        assert_eq!(incompatibilities(&old, &removed).len(), 1);
    }

    #[test]
    fn refuses_a_changed_mapping_key_or_value() {
        let old = sample();

        let mut key = old.clone();
        key["cells"]["0x00001000"]["keyType"] = json!({ "def": { "primitive": "u32" } });
        assert_eq!(incompatibilities(&old, &key).len(), 1);

        let mut value = old.clone();
        value["cells"]["0x00001000"]["value"]["leaf"]["def"] = json!({ "primitive": "u64" });
        assert_eq!(incompatibilities(&old, &value).len(), 1);
    }
}
