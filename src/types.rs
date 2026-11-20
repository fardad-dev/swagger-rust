//! Schemas → Rust types.
//!
//! Component schemas keep their names (`pet.Pet` → `PetPet`) and are only
//! generated when an endpoint reaches them. Inline objects and unions are
//! named after where they appear (`Pet` + `owner` → `PetOwner`); inline
//! enums after their field (`status` → `Status`) and shared by every field
//! with the same name and values, so a header enum repeated on 80
//! endpoints is one type.

use crate::config::PropertyNaming;
use crate::ir::*;
use crate::naming::{self, Names};
use crate::schema::{Additional, Schema};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Where an inline schema appears, to name the types it needs.
#[derive(Debug, Clone)]
pub struct Hint {
    parent: String,
    field: Option<String>,
    /// Whether the field name alone may name the type.
    short: bool,
}

impl Hint {
    /// A type named exactly `name`.
    pub fn exact(name: &str) -> Self {
        Self {
            parent: name.to_owned(),
            field: None,
            short: false,
        }
    }

    /// The `field` property of type `parent`.
    pub fn field(parent: &str, field: &str) -> Self {
        Self {
            parent: parent.to_owned(),
            field: Some(field.to_owned()),
            short: true,
        }
    }

    /// Like [`Hint::field`], but always prefixed with the parent
    /// (`PetVariant1`, never `Variant1`).
    fn prefixed(parent: &str, field: &str) -> Self {
        Self {
            parent: parent.to_owned(),
            field: Some(field.to_owned()),
            short: false,
        }
    }

    /// `Parent` + `Field`.
    fn long_name(&self) -> String {
        match &self.field {
            Some(field) => naming::type_ident(&format!("{}_{}", self.parent, field)),
            None => naming::type_ident(&self.parent),
        }
    }

    /// The names to try, in order: the field's own name (when allowed),
    /// then the parent-prefixed one.
    fn candidates(&self) -> Vec<String> {
        let long = self.long_name();
        match &self.field {
            Some(field) if self.short => {
                let short = naming::type_ident(field);
                if short == long {
                    vec![long]
                } else {
                    vec![short, long]
                }
            }
            _ => vec![long],
        }
    }

    fn suffixed(&self, suffix: &str) -> Self {
        match &self.field {
            Some(field) => Self {
                parent: self.parent.clone(),
                field: Some(format!("{field}_{suffix}")),
                short: self.short,
            },
            None => Self {
                parent: format!("{}_{suffix}", self.parent),
                field: None,
                short: false,
            },
        }
    }
}

/// What makes two inline structs the same type: their fields.
fn struct_shape(fields: &[Field]) -> Vec<(&str, &Ty, bool)> {
    fields
        .iter()
        .map(|f| (f.json.as_str(), &f.ty, f.optional))
        .collect()
}

fn same_struct(def: &Def, fields: &[Field]) -> bool {
    matches!(def, Def::Struct(existing) if !existing.input_only && struct_shape(&existing.fields) == struct_shape(fields))
}

fn same_union(def: &Def, union: &UnionDef) -> bool {
    match def {
        Def::Union(existing) => {
            existing.discriminator == union.discriminator
                && existing.variants.len() == union.variants.len()
                && existing
                    .variants
                    .iter()
                    .zip(&union.variants)
                    .all(|(a, b)| a.ty == b.ty && a.name == b.name)
        }
        _ => false,
    }
}

fn same_enum(def: &Def, e: &EnumDef) -> bool {
    match def {
        Def::Enum(existing) => {
            existing.kind == e.kind
                && existing.variants.len() == e.variants.len()
                && existing
                    .variants
                    .iter()
                    .zip(&e.variants)
                    .all(|(a, b)| a.1 == b.1)
        }
        _ => false,
    }
}

pub struct Types<'a> {
    document: &'a Value,
    schemas: serde_json::Map<String, Value>,
    pub names: Names,
    component_names: HashMap<String, String>,
    pub defs: BTreeMap<String, Def>,
    building: HashSet<String>,
    naming: PropertyNaming,
    /// Components that may be `null` wherever they are used.
    nullable_components: HashSet<String>,
    /// Structs some union decodes into; they get `FIELDS`/`REQUIRED`.
    pub union_members: HashSet<String>,
    pub warnings: Vec<String>,
}

fn wrap_nullable(ty: Ty, nullable: bool) -> Ty {
    if nullable && ty != Ty::Value {
        Ty::Opt(Box::new(ty))
    } else {
        ty
    }
}

fn primitive_of(value: &Value) -> Ty {
    match value {
        Value::String(_) => Ty::String,
        Value::Bool(_) => Ty::Bool,
        Value::Number(n) if n.is_i64() || n.is_u64() => Ty::I64,
        Value::Number(_) => Ty::F64,
        _ => Ty::Value,
    }
}

/// Decodes one JSON pointer segment (`~1`, `~0`, `%xx`).
fn decode_segment(segment: &str) -> String {
    let mut bytes = Vec::with_capacity(segment.len());
    let raw = segment.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            if let Ok(byte) = u8::from_str_radix(&segment[i + 1..i + 3], 16) {
                bytes.push(byte);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes)
        .replace("~1", "/")
        .replace("~0", "~")
}

impl<'a> Types<'a> {
    #[cfg(test)]
    pub fn new(document: &'a Value) -> Self {
        Self::with_naming(document, PropertyNaming::Document)
    }

    /// [`Types::new`], with model properties spelled as `naming` says on the
    /// wire.
    pub fn with_naming(document: &'a Value, naming: PropertyNaming) -> Self {
        let schemas = document
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut names = Names::with_reserved(naming::RESERVED_TYPES);
        let mut component_names = HashMap::new();
        for key in schemas.keys() {
            component_names.insert(key.clone(), names.unique(&naming::type_ident(key)));
        }
        Self {
            document,
            schemas,
            names,
            component_names,
            defs: BTreeMap::new(),
            building: HashSet::new(),
            naming,
            nullable_components: HashSet::new(),
            union_members: HashSet::new(),
            warnings: Vec::new(),
        }
    }

    /// How a property the document calls `name` is spelled on the wire.
    fn wire_name(&self, name: &str) -> String {
        match self.naming {
            PropertyNaming::Document => name.to_owned(),
            PropertyNaming::SnakeCase => naming::jackson_snake(name),
            PropertyNaming::CamelCase => naming::lower_camel(name),
        }
    }

    /// The type for `schema`, and whether it may be `null`.
    pub fn resolve(&mut self, schema: &Schema, hint: &Hint) -> (Ty, bool) {
        self.build(schema, hint, None)
    }

    /// Reserves a type name for a generated struct (query parameters…).
    pub fn reserve_name(&mut self, name: &str) -> String {
        self.names.unique(&naming::type_ident(name))
    }

    pub fn insert(&mut self, def: Def) {
        self.defs.insert(def.name().to_owned(), def);
    }

    /// Follows a local `$ref` to the schema it names.
    pub fn lookup(&self, reference: &str) -> Option<&'a Value> {
        let pointer = reference.strip_prefix('#')?;
        let decoded: Vec<String> = pointer.split('/').skip(1).map(decode_segment).collect();
        let mut node = self.document;
        for segment in &decoded {
            node = match node {
                Value::Object(map) => map.get(segment)?,
                Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(node)
    }

    /// The properties of an object schema, `$ref`s and `allOf` merged in.
    pub fn object_properties(&self, schema: &Schema) -> (Vec<(String, Schema)>, Vec<String>) {
        let mut properties = Vec::new();
        let mut required = Vec::new();
        let mut visited = HashSet::new();
        self.collect_properties(schema, &mut properties, &mut required, &mut visited);
        (properties, required)
    }

    fn collect_properties(
        &self,
        schema: &Schema,
        properties: &mut Vec<(String, Schema)>,
        required: &mut Vec<String>,
        visited: &mut HashSet<String>,
    ) {
        if let Some(reference) = &schema.reference {
            if visited.insert(reference.clone()) {
                if let Some(target) = self.lookup(reference) {
                    self.collect_properties(&Schema::parse(target), properties, required, visited);
                }
            }
        }
        for part in &schema.all_of {
            self.collect_properties(part, properties, required, visited);
        }
        for (name, property) in &schema.properties {
            match properties.iter_mut().find(|(existing, _)| existing == name) {
                Some(slot) => slot.1 = property.clone(),
                None => properties.push((name.clone(), property.clone())),
            }
        }
        for name in &schema.required {
            if !required.contains(name) {
                required.push(name.clone());
            }
        }
    }

    fn build(&mut self, s: &Schema, hint: &Hint, named: Option<&str>) -> (Ty, bool) {
        let nullable = s.nullable;
        if s.never {
            return (Ty::Value, true);
        }
        if let Some(reference) = &s.reference {
            if s.properties.is_empty() {
                let (ty, reference_nullable) = self.reference(reference, hint);
                return (ty, nullable || reference_nullable);
            }
        }

        if !s.all_of.is_empty() || (s.reference.is_some() && !s.properties.is_empty()) {
            let parts: Vec<&Schema> = s.all_of.iter().filter(|p| !p.is_any()).collect();
            if parts.len() == 1 && s.properties.is_empty() && s.reference.is_none() {
                let (ty, part_nullable) = self.build(parts[0], hint, named);
                return (ty, nullable || part_nullable);
            }
            let (properties, required) = self.object_properties(s);
            if properties.is_empty() {
                return (Ty::Object, nullable);
            }
            let name = self.define_struct(hint, named, s, properties, required);
            return (Ty::Named(name), nullable);
        }

        let alternatives = if !s.one_of.is_empty() {
            &s.one_of
        } else {
            &s.any_of
        };
        if !alternatives.is_empty() {
            let has_null = alternatives.iter().any(Schema::is_null_only);
            let options: Vec<&Schema> = alternatives.iter().filter(|a| !a.is_null_only()).collect();
            let nullable = nullable || has_null;
            if options.is_empty() {
                return (Ty::Value, true);
            }
            if options.len() == 1 && s.properties.is_empty() {
                let (ty, option_nullable) = self.build(options[0], hint, named);
                return (ty, nullable || option_nullable);
            }
            if !s.properties.is_empty() {
                // Properties next to a oneOf: the shared part is what can be typed.
                let (properties, required) = self.object_properties(s);
                let name = self.define_struct(hint, named, s, properties, required);
                return (Ty::Named(name), nullable);
            }
            let name = self.define_union(hint, named, s, &options);
            return (Ty::Named(name), nullable);
        }

        if let Some(constant) = &s.constant {
            return (primitive_of(constant), nullable);
        }
        if !s.enum_values.is_empty() {
            return self.build_enum(s, hint, named);
        }

        let ty = match s.types.as_slice() {
            [] => {
                if !s.properties.is_empty() || !matches!(s.additional, Additional::Absent) {
                    self.object(s, hint, named)
                } else if s.items.is_some() {
                    self.array(s, hint)
                } else {
                    Ty::Value
                }
            }
            [single] => match single.as_str() {
                "string" => Ty::String,
                "integer" => match s.format.as_deref() {
                    Some("int32") | Some("int16") | Some("int8") => Ty::I32,
                    _ => Ty::I64,
                },
                "number" => Ty::F64,
                "boolean" => Ty::Bool,
                "array" => self.array(s, hint),
                "object" => self.object(s, hint, named),
                _ => Ty::Value,
            },
            _ => Ty::Value,
        };
        (ty, nullable)
    }

    fn array(&mut self, s: &Schema, hint: &Hint) -> Ty {
        let (item, item_nullable) = match &s.items {
            Some(items) => self.build(items, &hint.suffixed("item"), None),
            None => (Ty::Value, false),
        };
        Ty::Vec(Box::new(wrap_nullable(item, item_nullable)))
    }

    fn object(&mut self, s: &Schema, hint: &Hint, named: Option<&str>) -> Ty {
        if !s.properties.is_empty() {
            let name = self.define_struct(hint, named, s, s.properties.clone(), s.required.clone());
            return Ty::Named(name);
        }
        match &s.additional {
            Additional::Schema(inner) if !inner.is_any() => {
                let (value, value_nullable) = self.build(inner, &hint.suffixed("value"), None);
                Ty::Map(Box::new(wrap_nullable(value, value_nullable)))
            }
            _ => Ty::Object,
        }
    }

    /// The name nested inline types are prefixed with, before the type's
    /// own name is settled: the candidate it will most likely get (free, or
    /// naming a type of the same shape from another endpoint). Identical
    /// nested types then get identical names and are shared.
    fn provisional_name(&self, hint: &Hint, named: Option<&str>) -> String {
        if let Some(name) = named {
            return name.to_owned();
        }
        let candidates = hint.candidates();
        candidates
            .iter()
            .find(|c| !self.names.is_taken(c) || self.defs.contains_key(*c))
            .cloned()
            .unwrap_or_else(|| hint.long_name())
    }

    /// Names an inline type: the first candidate that is free, or that
    /// already names a type of the same shape (which is then reused).
    /// Returns the name and whether it is reused.
    fn pick_name(&mut self, hint: &Hint, same: impl Fn(&Def) -> bool) -> (String, bool) {
        let candidates = hint.candidates();
        for candidate in &candidates {
            match self.defs.get(candidate) {
                Some(existing) if same(existing) => return (candidate.clone(), true),
                Some(_) => continue,
                None if !self.names.is_taken(candidate) => {
                    self.names.reserve(candidate);
                    return (candidate.clone(), false);
                }
                None => continue,
            }
        }
        (
            self.names.unique(candidates.last().expect("a candidate")),
            false,
        )
    }

    fn reference(&mut self, reference: &str, hint: &Hint) -> (Ty, bool) {
        if let Some(key) = reference.strip_prefix("#/components/schemas/") {
            let key = decode_segment(key);
            if let Some(rust) = self.component_names.get(&key).cloned() {
                self.ensure_component(&key, &rust);
                let nullable = self.nullable_components.contains(&rust);
                return (Ty::Named(rust), nullable);
            }
        }
        if reference.starts_with("#/") {
            if let Some(target) = self.lookup(reference) {
                let schema = Schema::parse(target);
                if schema.reference.as_deref() != Some(reference) {
                    return self.build(&schema, hint, None);
                }
            }
            self.warn(format!(
                "`{reference}` points nowhere; typed as serde_json::Value"
            ));
        } else {
            self.warn(format!(
                "external reference `{reference}` is not supported; typed as serde_json::Value"
            ));
        }
        (Ty::Value, false)
    }

    fn ensure_component(&mut self, key: &str, rust: &str) {
        if self.defs.contains_key(rust) || self.building.contains(rust) {
            return;
        }
        self.building.insert(rust.to_owned());
        let schema = Schema::parse(&self.schemas[key]);
        let (ty, nullable) = self.build(&schema, &Hint::exact(rust), Some(rust));
        if nullable {
            self.nullable_components.insert(rust.to_owned());
        }
        if ty != Ty::Named(rust.to_owned()) {
            self.defs.insert(
                rust.to_owned(),
                Def::Alias(AliasDef {
                    name: rust.to_owned(),
                    doc: schema.doc(),
                    ty,
                }),
            );
        }
        self.building.remove(rust);
    }

    /// Defines a struct (named `named`, or after `hint`) and returns its
    /// name.
    fn define_struct(
        &mut self,
        hint: &Hint,
        named: Option<&str>,
        s: &Schema,
        properties: Vec<(String, Schema)>,
        required: Vec<String>,
    ) -> String {
        let parent = self.provisional_name(hint, named);
        let mut field_names = Names::new();
        let mut fields = Vec::with_capacity(properties.len());
        for (json, property) in properties {
            let (ty, nullable) = self.build(&property, &Hint::field(&parent, &json), None);
            let optional = nullable || !required.contains(&json);
            let wire = self.wire_name(&json);
            let alias = (wire != json).then(|| json.clone());
            fields.push(Field {
                name: field_names.unique(&naming::value_ident(&wire)),
                json: wire,
                ty,
                optional,
                boxed: false,
                doc: property.doc(),
                deprecated: property.deprecated,
                flatten: false,
                minimum: property.minimum,
                alias,
            });
        }
        let name = match named {
            Some(name) => name.to_owned(),
            None => {
                let (name, reused) =
                    self.pick_name(hint, |existing| same_struct(existing, &fields));
                if reused {
                    return name;
                }
                name
            }
        };
        self.defs.insert(
            name.clone(),
            Def::Struct(StructDef {
                name: name.clone(),
                doc: s.doc(),
                fields,
                input_only: false,
                deprecated: s.deprecated,
            }),
        );
        name
    }

    /// Defines a union (named `named`, or after `hint`) and returns its name.
    fn define_union(
        &mut self,
        hint: &Hint,
        named: Option<&str>,
        s: &Schema,
        options: &[&Schema],
    ) -> String {
        let parent = self.provisional_name(hint, named);
        let mut variant_names = Names::with_reserved(&["Unknown"]);
        let mut variants = Vec::with_capacity(options.len());
        for (index, option) in options.iter().enumerate() {
            let (ty, _) = self.build(
                option,
                &Hint::prefixed(&parent, &format!("variant{}", index + 1)),
                None,
            );
            let base = match &ty {
                Ty::Named(name) => name.clone(),
                Ty::String => "String".into(),
                Ty::Bool => "Boolean".into(),
                Ty::I32 | Ty::I64 => "Integer".into(),
                Ty::F64 => "Number".into(),
                Ty::Vec(_) => "Array".into(),
                Ty::Object | Ty::Map(_) => "Object".into(),
                _ => "Value".into(),
            };
            variants.push(UnionVariant {
                name: variant_names.unique(&base),
                ty,
                boxed: false,
            });
        }

        let discriminator = s.discriminator.as_ref().map(|discriminator| {
            let mut mapping = Vec::new();
            let variant_of = |rust: &str| variants.iter().position(|v| v.ty.named() == Some(rust));
            if discriminator.mapping.is_empty() {
                for (key, rust) in &self.component_names {
                    if let Some(index) = variant_of(rust) {
                        mapping.push((key.clone(), index));
                    }
                }
                mapping.sort();
            } else {
                for (value, reference) in &discriminator.mapping {
                    let key = reference
                        .strip_prefix("#/components/schemas/")
                        .map(decode_segment);
                    let rust = key
                        .as_ref()
                        .and_then(|k| self.component_names.get(k))
                        .cloned()
                        .unwrap_or_else(|| {
                            // A bare schema name is allowed too.
                            self.component_names
                                .get(reference)
                                .cloned()
                                .unwrap_or_default()
                        });
                    if let Some(index) = variant_of(&rust) {
                        mapping.push((value.clone(), index));
                    }
                }
            }
            (self.wire_name(&discriminator.property), mapping)
        });

        for variant in &variants {
            if let Ty::Named(member) = &variant.ty {
                self.union_members.insert(member.clone());
            }
        }
        let mut union = UnionDef {
            name: String::new(),
            doc: s.doc(),
            variants,
            discriminator,
        };
        let name = match named {
            Some(name) => name.to_owned(),
            None => {
                let (name, reused) = self.pick_name(hint, |existing| same_union(existing, &union));
                if reused {
                    return name;
                }
                name
            }
        };
        union.name = name.clone();
        self.defs.insert(name.clone(), Def::Union(union));
        name
    }

    fn build_enum(&mut self, s: &Schema, hint: &Hint, named: Option<&str>) -> (Ty, bool) {
        let mut values: Vec<Value> = Vec::with_capacity(s.enum_values.len());
        for value in &s.enum_values {
            if !values.contains(value) {
                values.push(value.clone());
            }
        }
        let values = &values;
        let kind = if values.iter().all(Value::is_string) {
            EnumKind::String
        } else if values.iter().all(|v| v.as_i64().is_some()) {
            EnumKind::Integer
        } else {
            let ty = match s.types.first().map(String::as_str) {
                Some("string") => Ty::String,
                Some("integer") => Ty::I64,
                Some("number") => Ty::F64,
                Some("boolean") => Ty::Bool,
                _ => Ty::Value,
            };
            return (ty, s.nullable);
        };
        if values.len() == 1 && named.is_none() {
            let ty = match kind {
                EnumKind::String => Ty::String,
                EnumKind::Integer => Ty::I64,
            };
            return (ty, s.nullable);
        }

        let use_names = s.enum_names.len() == values.len() && values.len() == s.enum_values.len();
        let mut variant_names = Names::with_reserved(&["Unknown"]);
        let variants: Vec<(String, Value)> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let base = if use_names {
                    naming::variant_ident(&s.enum_names[index])
                } else {
                    match value {
                        Value::String(text) => naming::variant_ident(text),
                        other => match other.as_i64() {
                            Some(n) if n < 0 => format!("Minus{}", n.unsigned_abs()),
                            Some(n) => format!("V{n}"),
                            None => "Value".to_owned(),
                        },
                    }
                };
                (variant_names.unique(&base), value.clone())
            })
            .collect();

        let mut def = EnumDef {
            name: String::new(),
            doc: s.doc(),
            kind,
            variants,
        };
        let name = match named {
            Some(name) => name.to_owned(),
            None => {
                // Enums may take the field's own name even for exact hints
                // (`status` → `Status`).
                let hint = Hint {
                    short: true,
                    ..hint.clone()
                };
                let (name, reused) = self.pick_name(&hint, |existing| same_enum(existing, &def));
                if reused {
                    return (Ty::Named(name), s.nullable);
                }
                name
            }
        };
        def.name = name.clone();
        self.defs.entry(name.clone()).or_insert(Def::Enum(def));
        (Ty::Named(name), s.nullable)
    }

    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// Breaks recursive types: boxes struct fields and union variants that
    /// lead back to their own type, and turns aliases that only refer to
    /// themselves (which Rust can't express) into `serde_json::Value`.
    pub fn finish(&mut self) {
        // Alias cycles first.
        let alias_names: Vec<String> = self
            .defs
            .values()
            .filter_map(|d| {
                if let Def::Alias(a) = d {
                    Some(a.name.clone())
                } else {
                    None
                }
            })
            .collect();
        for name in alias_names {
            if self.alias_reaches(&name, &name, &mut HashSet::new()) {
                if let Some(Def::Alias(alias)) = self.defs.get_mut(&name) {
                    alias.ty = Ty::Value;
                }
                self.warn(format!(
                    "type alias `{name}` refers to itself; typed as serde_json::Value"
                ));
            }
        }

        let edges: HashMap<String, Vec<String>> = self
            .defs
            .iter()
            .map(|(name, def)| (name.clone(), direct_edges(def)))
            .collect();
        let reaches = |from: &str, to: &str| -> bool {
            let mut seen = HashSet::new();
            let mut stack = vec![from.to_owned()];
            while let Some(node) = stack.pop() {
                if node == to {
                    return true;
                }
                if seen.insert(node.clone()) {
                    if let Some(next) = edges.get(&node) {
                        stack.extend(next.iter().cloned());
                    }
                }
            }
            false
        };
        let mut boxes: Vec<(String, usize)> = Vec::new();
        for (name, def) in &self.defs {
            match def {
                Def::Struct(s) => {
                    for (index, field) in s.fields.iter().enumerate() {
                        if let Ty::Named(target) = &field.ty {
                            if reaches(target, name) {
                                boxes.push((name.clone(), index));
                            }
                        }
                    }
                }
                Def::Union(u) => {
                    for (index, variant) in u.variants.iter().enumerate() {
                        if let Ty::Named(target) = &variant.ty {
                            if reaches(target, name) {
                                boxes.push((name.clone(), index));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for (name, index) in boxes {
            match self.defs.get_mut(&name) {
                Some(Def::Struct(s)) => s.fields[index].boxed = true,
                Some(Def::Union(u)) => u.variants[index].boxed = true,
                _ => {}
            }
        }
    }

    fn alias_reaches(&self, from: &str, target: &str, seen: &mut HashSet<String>) -> bool {
        let Some(Def::Alias(alias)) = self.defs.get(from) else {
            return false;
        };
        let mut found = false;
        visit_named(&alias.ty, &mut |name| {
            if name == target
                || (seen.insert(name.to_owned()) && self.alias_reaches(name, target, seen))
            {
                found = true;
            }
        });
        found
    }
}

/// Types a def contains inline (not behind a `Vec` or map).
fn direct_edges(def: &Def) -> Vec<String> {
    match def {
        Def::Struct(s) => s
            .fields
            .iter()
            .filter_map(|f| f.ty.named().map(str::to_owned))
            .collect(),
        Def::Union(u) => u
            .variants
            .iter()
            .filter_map(|v| v.ty.named().map(str::to_owned))
            .collect(),
        Def::Alias(a) => match &a.ty {
            Ty::Named(name) => vec![name.clone()],
            Ty::Opt(inner) => inner
                .named()
                .map(|n| vec![n.to_owned()])
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        Def::Enum(_) => Vec::new(),
    }
}

pub fn visit_named(ty: &Ty, visit: &mut impl FnMut(&str)) {
    match ty {
        Ty::Named(name) => visit(name),
        Ty::Map(inner) | Ty::Vec(inner) | Ty::Opt(inner) => visit_named(inner, visit),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn types_for(document: &Value) -> Types<'_> {
        Types::new(document)
    }

    #[test]
    fn components_inline_objects_enums_and_recursion() {
        let document = json!({
            "components": {"schemas": {
                "Pet": {
                    "type": "object",
                    "required": ["id", "status"],
                    "properties": {
                        "id": {"type": "integer", "format": "int64"},
                        "status": {"type": "string", "enum": ["available", "sold"]},
                        "owner": {"type": "object", "properties": {"name": {"type": "string"}}},
                        "parent": {"$ref": "#/components/schemas/Pet"},
                        "children": {"type": "array", "items": {"$ref": "#/components/schemas/Pet"}},
                        "tags": {"type": "object", "additionalProperties": {"type": "string"}}
                    }
                },
                "Unused": {"type": "object", "properties": {"x": {"type": "string"}}}
            }}
        });
        let mut types = types_for(&document);
        let (ty, nullable) = types.resolve(
            &Schema::parse(&json!({"$ref": "#/components/schemas/Pet"})),
            &Hint::exact("X"),
        );
        types.finish();
        assert_eq!(ty, Ty::Named("Pet".into()));
        assert!(!nullable);
        assert!(!types.defs.contains_key("Unused"));
        let Def::Struct(pet) = &types.defs["Pet"] else {
            panic!()
        };
        let field = |name: &str| pet.fields.iter().find(|f| f.name == name).unwrap();
        assert_eq!(field("id").ty, Ty::I64);
        assert!(!field("id").optional);
        assert_eq!(field("status").ty, Ty::Named("Status".into()));
        assert_eq!(field("owner").ty, Ty::Named("Owner".into()));
        assert!(field("parent").boxed && field("parent").optional);
        assert!(!field("children").boxed);
        assert_eq!(field("tags").ty, Ty::Map(Box::new(Ty::String)));
    }

    #[test]
    fn same_enum_is_shared_and_different_one_is_prefixed() {
        let document = json!({});
        let mut types = types_for(&document);
        let site = json!({"type": "string", "enum": ["A", "B"]});
        let other = json!({"type": "string", "enum": ["C"] });
        let (a, _) = types.resolve(
            &Schema::parse(&site),
            &Hint::field("OpOneHeaderParams", "x-site"),
        );
        let (b, _) = types.resolve(
            &Schema::parse(&site),
            &Hint::field("OpTwoHeaderParams", "x-site"),
        );
        let (c, _) = types.resolve(
            &Schema::parse(&json!({"type": "string", "enum": ["C", "D"]})),
            &Hint::field("OpThreeHeaderParams", "x-site"),
        );
        let (single, _) = types.resolve(&Schema::parse(&other), &Hint::field("P", "kind"));
        assert_eq!(a, Ty::Named("XSite".into()));
        assert_eq!(b, a);
        assert_eq!(c, Ty::Named("OpThreeHeaderParamsXSite".into()));
        assert_eq!(single, Ty::String);
    }

    #[test]
    fn unions_nullable_and_all_of() {
        let document = json!({"components": {"schemas": {
            "Base": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}},
            "Cat": {"allOf": [{"$ref": "#/components/schemas/Base"}, {"type": "object", "properties": {"meow": {"type": "boolean"}}}]},
            "Dog": {"type": "object", "properties": {"bark": {"type": "boolean"}}},
            "Pet": {"oneOf": [{"$ref": "#/components/schemas/Cat"}, {"$ref": "#/components/schemas/Dog"}],
                    "discriminator": {"propertyName": "kind", "mapping": {"cat": "#/components/schemas/Cat"}}},
            "MaybeDog": {"oneOf": [{"$ref": "#/components/schemas/Dog"}, {"type": "null"}]}
        }}});
        let mut types = types_for(&document);
        types.resolve(
            &Schema::parse(&json!({"$ref": "#/components/schemas/Pet"})),
            &Hint::exact("X"),
        );
        let (maybe, nullable) = types.resolve(
            &Schema::parse(&json!({"$ref": "#/components/schemas/MaybeDog"})),
            &Hint::exact("Y"),
        );
        assert_eq!(maybe, Ty::Named("MaybeDog".into()));
        assert!(
            nullable,
            "a oneOf with null makes every use of the component optional"
        );
        let Def::Struct(cat) = &types.defs["Cat"] else {
            panic!()
        };
        assert_eq!(
            cat.fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["id", "meow"]
        );
        assert!(!cat.fields[0].optional);
        let Def::Union(pet) = &types.defs["Pet"] else {
            panic!()
        };
        assert_eq!(
            pet.variants
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>(),
            ["Cat", "Dog"]
        );
        assert_eq!(
            pet.discriminator.as_ref().unwrap().1,
            vec![("cat".to_owned(), 0)]
        );
        assert!(types.union_members.contains("Cat"));
    }
}
