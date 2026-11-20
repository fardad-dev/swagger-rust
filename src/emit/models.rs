//! `models.rs`: structs, enums, unions and aliases.

use super::{doc_comment, generated_header, string_literal, Context, ALLOW};
use crate::ir::*;
use crate::types::Types;

pub fn render(cx: &Context, types: &Types) -> String {
    let lenient = cx.lenient();
    let mut out = generated_header();
    out.push_str("//! The request and response types.\n");
    out.push_str(ALLOW);
    out.push('\n');
    if lenient {
        out.push_str("use super::json::{self, FromJson};\nuse serde::Serialize;\n");
    } else {
        out.push_str("use super::json;\nuse serde::{Deserialize, Serialize};\n");
    }
    out.push_str("use serde_json::Value;\n");

    let mut decodable = Vec::new();
    for def in types.defs.values() {
        out.push('\n');
        match def {
            Def::Struct(s) => {
                out.push_str(&structure(s, lenient));
                if types.union_members.contains(&s.name) {
                    out.push_str(&field_consts(s));
                }
                if !s.input_only {
                    decodable.push(s.name.clone());
                }
            }
            Def::Enum(e) => {
                out.push_str(&enumeration(e, lenient));
                decodable.push(e.name.clone());
            }
            Def::Union(u) => {
                out.push_str(&union(u, types, lenient));
                decodable.push(u.name.clone());
            }
            Def::Alias(a) => {
                if let Some(doc) = &a.doc {
                    out.push_str(&doc_comment(doc, "", false));
                }
                out.push_str(&format!(
                    "pub type {} = {};\n",
                    a.name,
                    a.ty.rust(Scope::Models)
                ));
            }
        }
    }

    if lenient && !decodable.is_empty() {
        out.push_str("\njson::deserialize_via_from_json!(\n");
        for name in &decodable {
            out.push_str(&format!("    {name},\n"));
        }
        out.push_str(");\n");
    }
    out
}

fn structure(s: &StructDef, lenient: bool) -> String {
    let mut out = String::new();
    if let Some(doc) = &s.doc {
        out.push_str(&doc_comment(doc, "", false));
    }
    if s.deprecated {
        if s.doc.is_some() {
            out.push_str("///\n");
        }
        out.push_str("/// **Deprecated.**\n");
    }
    let derives = if s.input_only || lenient {
        "Debug, Clone, Default, PartialEq, Serialize"
    } else {
        "Debug, Clone, Default, PartialEq, Serialize, Deserialize"
    };
    out.push_str(&format!("#[derive({derives})]\npub struct {} {{\n", s.name));
    for field in &s.fields {
        if let Some(doc) = &field.doc {
            out.push_str(&doc_comment(doc, "    ", false));
        }
        if field.deprecated {
            if field.doc.is_some() {
                out.push_str("    ///\n");
            }
            out.push_str("    /// **Deprecated.**\n");
        }
        let mut attributes = Vec::new();
        if field.flatten {
            attributes.push("flatten".to_owned());
        } else if field.name != field.json {
            attributes.push(format!("rename = {}", string_literal(&field.json)));
        }
        if field.optional {
            if !lenient && !s.input_only {
                attributes.push("default".to_owned());
            }
            attributes.push("skip_serializing_if = \"Option::is_none\"".to_owned());
        }
        if !attributes.is_empty() {
            out.push_str(&format!("    #[serde({})]\n", attributes.join(", ")));
        }
        out.push_str(&format!(
            "    pub {}: {},\n",
            field.name,
            field.rust(Scope::Models)
        ));
    }
    out.push_str("}\n");

    if lenient && !s.input_only {
        out.push_str(&format!("\nimpl FromJson for {} {{\n", s.name));
        out.push_str("    fn from_json(value: Value) -> Option<Self> {\n");
        if s.fields.is_empty() {
            out.push_str("        json::object(value)?;\n        Some(Self {})\n");
        } else {
            out.push_str("        let mut object = json::object(value)?;\n        Some(Self {\n");
            for field in &s.fields {
                let reader = if field.optional { "optional" } else { "field" };
                out.push_str(&format!(
                    "            {}: json::{reader}(&mut object, {}),\n",
                    field.name,
                    string_literal(&field.json)
                ));
            }
            out.push_str("        })\n");
        }
        out.push_str("    }\n}\n");
    }
    out
}

/// `FIELDS`/`REQUIRED`, which unions use to pick a variant.
fn field_consts(s: &StructDef) -> String {
    let list = |fields: Vec<&Field>| {
        fields
            .iter()
            .map(|f| string_literal(&f.json))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "\nimpl {} {{\n    /// The JSON field names.\n    pub const FIELDS: &'static [&'static str] = &[{}];\n    /// The JSON fields that must be present.\n    pub const REQUIRED: &'static [&'static str] = &[{}];\n}}\n",
        s.name,
        list(s.fields.iter().collect()),
        list(s.fields.iter().filter(|f| !f.optional).collect()),
    )
}

fn enumeration(e: &EnumDef, lenient: bool) -> String {
    let mut out = String::new();
    if let Some(doc) = &e.doc {
        out.push_str(&doc_comment(doc, "", false));
    }
    let name = &e.name;
    match e.kind {
        EnumKind::String => {
            out.push_str(&format!(
                "#[derive(Debug, Clone, PartialEq, Eq, Hash)]\npub enum {name} {{\n"
            ));
            for (variant, value) in &e.variants {
                let text = value.as_str().unwrap_or_default();
                out.push_str(&format!(
                    "    /// `{}`\n    {variant},\n",
                    text.replace('`', "'")
                ));
            }
            out.push_str("    /// A value this client doesn't know (added to the API later).\n    Unknown(String),\n}\n");
            out.push_str(&format!(
                "\nimpl {name} {{\n    pub fn as_str(&self) -> &str {{\n        match self {{\n"
            ));
            for (variant, value) in &e.variants {
                out.push_str(&format!(
                    "            Self::{variant} => {},\n",
                    string_literal(value.as_str().unwrap_or_default())
                ));
            }
            out.push_str("            Self::Unknown(value) => value,\n        }\n    }\n}\n");
            out.push_str(&format!(
                "\nimpl Default for {name} {{\n    fn default() -> Self {{\n        Self::Unknown(String::new())\n    }}\n}}\n"
            ));
            out.push_str(&format!("\nimpl From<&str> for {name} {{\n    fn from(value: &str) -> Self {{\n        match value {{\n"));
            for (variant, value) in &e.variants {
                out.push_str(&format!(
                    "            {} => Self::{variant},\n",
                    string_literal(value.as_str().unwrap_or_default())
                ));
            }
            out.push_str(
                "            other => Self::Unknown(other.to_owned()),\n        }\n    }\n}\n",
            );
            out.push_str(&format!(
                "\nimpl From<String> for {name} {{\n    fn from(value: String) -> Self {{\n        Self::from(value.as_str())\n    }}\n}}\n"
            ));
            out.push_str(&format!(
                "\nimpl std::fmt::Display for {name} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n        f.write_str(self.as_str())\n    }}\n}}\n"
            ));
            out.push_str(&format!(
                "\nimpl Serialize for {name} {{\n    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {{\n        serializer.serialize_str(self.as_str())\n    }}\n}}\n"
            ));
            if lenient {
                out.push_str(&format!(
                    "\nimpl FromJson for {name} {{\n    fn from_json(value: Value) -> Option<Self> {{\n        json::text(value).map(Self::from)\n    }}\n}}\n"
                ));
            } else {
                out.push_str(&format!(
                    "\nimpl<'de> Deserialize<'de> for {name} {{\n    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{\n        String::deserialize(deserializer).map(Self::from)\n    }}\n}}\n"
                ));
            }
        }
        EnumKind::Integer => {
            out.push_str(&format!(
                "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\npub enum {name} {{\n"
            ));
            for (variant, value) in &e.variants {
                out.push_str(&format!("    /// `{value}`\n    {variant},\n"));
            }
            out.push_str("    /// A value this client doesn't know (added to the API later).\n    Unknown(i64),\n}\n");
            out.push_str(&format!(
                "\nimpl {name} {{\n    pub fn as_i64(&self) -> i64 {{\n        match self {{\n"
            ));
            for (variant, value) in &e.variants {
                out.push_str(&format!("            Self::{variant} => {value},\n"));
            }
            out.push_str("            Self::Unknown(value) => *value,\n        }\n    }\n}\n");
            out.push_str(&format!(
                "\nimpl Default for {name} {{\n    fn default() -> Self {{\n        Self::Unknown(0)\n    }}\n}}\n"
            ));
            out.push_str(&format!("\nimpl From<i64> for {name} {{\n    fn from(value: i64) -> Self {{\n        match value {{\n"));
            for (variant, value) in &e.variants {
                out.push_str(&format!("            {value} => Self::{variant},\n"));
            }
            out.push_str("            other => Self::Unknown(other),\n        }\n    }\n}\n");
            out.push_str(&format!(
                "\nimpl std::fmt::Display for {name} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n        write!(f, \"{{}}\", self.as_i64())\n    }}\n}}\n"
            ));
            out.push_str(&format!(
                "\nimpl Serialize for {name} {{\n    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {{\n        serializer.serialize_i64(self.as_i64())\n    }}\n}}\n"
            ));
            if lenient {
                out.push_str(&format!(
                    "\nimpl FromJson for {name} {{\n    fn from_json(value: Value) -> Option<Self> {{\n        i64::from_json(value).map(Self::from)\n    }}\n}}\n"
                ));
            } else {
                out.push_str(&format!(
                    "\nimpl<'de> Deserialize<'de> for {name} {{\n    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{\n        i64::deserialize(deserializer).map(Self::from)\n    }}\n}}\n"
                ));
            }
        }
    }
    out
}

/// The `json::Shape` a union variant is scored with.
fn shape(ty: &Ty, types: &Types, depth: usize) -> String {
    match ty {
        Ty::String => "json::Shape::String".into(),
        Ty::I32 | Ty::I64 => "json::Shape::Integer".into(),
        Ty::F64 => "json::Shape::Number".into(),
        Ty::Bool => "json::Shape::Bool".into(),
        Ty::Vec(_) => "json::Shape::Array".into(),
        Ty::Object | Ty::Map(_) => "json::Shape::Object(&[], &[])".into(),
        Ty::Named(name) if depth < 8 => match types.defs.get(name) {
            Some(Def::Struct(_)) => {
                format!("json::Shape::Object({name}::FIELDS, {name}::REQUIRED)")
            }
            Some(Def::Enum(e)) if e.kind == EnumKind::String => "json::Shape::String".into(),
            Some(Def::Enum(_)) => "json::Shape::Integer".into(),
            Some(Def::Alias(a)) => shape(&a.ty, types, depth + 1),
            _ => "json::Shape::Any".into(),
        },
        _ => "json::Shape::Any".into(),
    }
}

fn union(u: &UnionDef, types: &Types, lenient: bool) -> String {
    let name = &u.name;
    let mut out = String::new();
    if let Some(doc) = &u.doc {
        out.push_str(&doc_comment(doc, "", false));
    }
    out.push_str(&format!(
        "#[derive(Debug, Clone, PartialEq)]\npub enum {name} {{\n"
    ));
    for variant in &u.variants {
        let mut ty = variant.ty.rust(Scope::Models);
        if variant.boxed {
            ty = format!("Box<{ty}>");
        }
        out.push_str(&format!("    {}({ty}),\n", variant.name));
    }
    out.push_str("    /// A value matching none of the variants.\n    Unknown(Value),\n}\n");

    out.push_str(&format!(
        "\nimpl Default for {name} {{\n    fn default() -> Self {{\n        Self::Unknown(Value::Null)\n    }}\n}}\n"
    ));
    out.push_str(&format!(
        "\nimpl Serialize for {name} {{\n    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {{\n        match self {{\n"
    ));
    for variant in &u.variants {
        out.push_str(&format!(
            "            Self::{}(value) => value.serialize(serializer),\n",
            variant.name
        ));
    }
    out.push_str(
        "            Self::Unknown(value) => value.serialize(serializer),\n        }\n    }\n}\n",
    );

    let construct = |variant: &UnionVariant| -> String {
        if variant.boxed {
            format!("|value| Self::{}(Box::new(value))", variant.name)
        } else {
            format!("Self::{}", variant.name)
        }
    };
    out.push_str(&format!("\nimpl {name} {{\n"));
    out.push_str("    /// The variant `value` is: by its discriminator, or the best fit.\n");
    out.push_str("    pub fn from_value(value: Value) -> Result<Self, String> {\n");
    match &u.discriminator {
        Some((property, mapping)) if !mapping.is_empty() => {
            out.push_str(&format!(
                "        match json::tag(&value, {}).as_deref() {{\n",
                string_literal(property)
            ));
            for (tag, index) in mapping {
                let variant = &u.variants[*index];
                out.push_str(&format!(
                    "            Some({}) => json::decode(value).map({}),\n",
                    string_literal(tag),
                    construct(variant)
                ));
            }
            out.push_str("            _ => Ok(Self::Unknown(value)),\n        }\n");
        }
        _ => {
            out.push_str("        let scores = [\n");
            for variant in &u.variants {
                out.push_str(&format!(
                    "            json::score(&value, &{}),\n",
                    shape(&variant.ty, types, 0)
                ));
            }
            out.push_str("        ];\n        for index in json::ranked(&scores) {\n            let decoded = match index {\n");
            for (index, variant) in u.variants.iter().enumerate() {
                out.push_str(&format!(
                    "                {index} => json::decode(value.clone()).map({}),\n",
                    construct(variant)
                ));
            }
            out.push_str("                _ => continue,\n            };\n");
            out.push_str("            if let Ok(decoded) = decoded {\n                return Ok(decoded);\n            }\n        }\n");
            out.push_str("        Ok(Self::Unknown(value))\n");
        }
    }
    out.push_str("    }\n}\n");

    if lenient {
        out.push_str(&format!(
            "\nimpl FromJson for {name} {{\n    fn from_json(value: Value) -> Option<Self> {{\n        Self::from_value(value).ok()\n    }}\n}}\n"
        ));
    } else {
        out.push_str(&format!(
            "\nimpl<'de> Deserialize<'de> for {name} {{\n    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{\n        let value = Value::deserialize(deserializer)?;\n        Self::from_value(value).map_err(serde::de::Error::custom)\n    }}\n}}\n"
        ));
    }
    out
}
