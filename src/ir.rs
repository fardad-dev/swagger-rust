//! What gets generated, independent of how it is printed.

use serde_json::Value;

/// A Rust type.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    String,
    Bool,
    I32,
    I64,
    F64,
    /// Any JSON (`serde_json::Value`).
    Value,
    /// A free-form JSON object (`serde_json::Map<String, Value>`).
    Object,
    /// `HashMap<String, T>`.
    Map(Box<Ty>),
    Vec(Box<Ty>),
    /// An element that may be `null` (inside a `Vec` or `Map`).
    Opt(Box<Ty>),
    /// A generated model.
    Named(String),
    /// An uploaded file (`runtime::FilePart`), multipart bodies only.
    File,
}

impl Ty {
    pub fn named(&self) -> Option<&str> {
        match self {
            Ty::Named(name) => Some(name),
            _ => None,
        }
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Ty::I32 | Ty::I64)
    }
}

/// How to print a [`Ty`]: from `models.rs` itself (models unqualified) or
/// from a sibling module (`models::Pet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Models,
    Sibling,
}

impl Ty {
    pub fn rust(&self, scope: Scope) -> String {
        match self {
            Ty::String => "String".into(),
            Ty::Bool => "bool".into(),
            Ty::I32 => "i32".into(),
            Ty::I64 => "i64".into(),
            Ty::F64 => "f64".into(),
            Ty::Value => "serde_json::Value".into(),
            Ty::Object => "serde_json::Map<String, serde_json::Value>".into(),
            Ty::Map(inner) => format!("std::collections::HashMap<String, {}>", inner.rust(scope)),
            Ty::Vec(inner) => format!("Vec<{}>", inner.rust(scope)),
            Ty::Opt(inner) => format!("Option<{}>", inner.rust(scope)),
            Ty::Named(name) => match scope {
                Scope::Models => name.clone(),
                Scope::Sibling => format!("models::{name}"),
            },
            Ty::File => match scope {
                Scope::Models => "super::runtime::FilePart".into(),
                Scope::Sibling => "runtime::FilePart".into(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    /// The JSON property name.
    pub json: String,
    /// The Rust field name.
    pub name: String,
    pub ty: Ty,
    /// `Option<ty>`: not required, or nullable.
    pub optional: bool,
    /// Boxed to break a recursive type.
    pub boxed: bool,
    pub doc: Option<String>,
    pub deprecated: bool,
}

impl Field {
    /// The field's full type.
    pub fn rust(&self, scope: Scope) -> String {
        let mut ty = self.ty.rust(scope);
        if self.boxed {
            ty = format!("Box<{ty}>");
        }
        if self.optional {
            ty = format!("Option<{ty}>");
        }
        ty
    }
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub doc: Option<String>,
    pub fields: Vec<Field>,
    /// Request-only types (query and header parameters, multipart bodies):
    /// serialized, never decoded.
    pub input_only: bool,
    pub deprecated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumKind {
    String,
    Integer,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub doc: Option<String>,
    pub kind: EnumKind,
    /// Variant name → value.
    pub variants: Vec<(String, Value)>,
}

#[derive(Debug, Clone)]
pub struct UnionVariant {
    pub name: String,
    pub ty: Ty,
    pub boxed: bool,
}

#[derive(Debug, Clone)]
pub struct UnionDef {
    pub name: String,
    pub doc: Option<String>,
    pub variants: Vec<UnionVariant>,
    /// The property telling the variants apart, and value → variant index.
    pub discriminator: Option<(String, Vec<(String, usize)>)>,
}

#[derive(Debug, Clone)]
pub struct AliasDef {
    pub name: String,
    pub doc: Option<String>,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub enum Def {
    Struct(StructDef),
    Enum(EnumDef),
    Union(UnionDef),
    Alias(AliasDef),
}

impl Def {
    pub fn name(&self) -> &str {
        match self {
            Def::Struct(d) => &d.name,
            Def::Enum(d) => &d.name,
            Def::Union(d) => &d.name,
            Def::Alias(d) => &d.name,
        }
    }
}

/// A parameter of the generated function that fills a `{placeholder}`.
#[derive(Debug, Clone)]
pub struct PathParam {
    pub json: String,
    pub name: String,
    pub ty: Ty,
    pub doc: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MultipartKind {
    /// A scalar, sent as text.
    Text,
    /// An object or array, sent as JSON text.
    Json,
    File,
    Files,
}

#[derive(Debug, Clone)]
pub struct MultipartField {
    pub json: String,
    pub name: String,
    pub kind: MultipartKind,
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub enum Body {
    Json {
        ty: Ty,
        content_type: String,
    },
    Form {
        ty: Ty,
    },
    Multipart {
        type_name: String,
        fields: Vec<MultipartField>,
    },
    Text {
        content_type: String,
    },
    Bytes {
        content_type: String,
    },
}

#[derive(Debug, Clone)]
pub enum Response {
    Json(Ty),
    Text,
    Bytes,
    Empty,
}

#[derive(Debug, Clone)]
pub enum QueryKind {
    /// No GPUI query (a mutation, or GPUI is off).
    None,
    Query,
    /// Pages through `page_field` of the query parameters.
    Infinite {
        page_field: String,
        page_ty: Ty,
        optional: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Operation {
    /// swagger-typescript's name (`getPetsId`), what config lists match.
    pub ts_name: String,
    /// The Rust function name (`get_pets_id`).
    pub fn_name: String,
    /// Lower case (`get`).
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub path_params: Vec<PathParam>,
    /// The query parameters struct.
    pub query: Option<String>,
    /// The header parameters struct.
    pub headers: Option<String>,
    pub body: Option<Body>,
    pub response: Response,
    pub query_kind: QueryKind,
}
