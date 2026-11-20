//! The parts of a JSON Schema / OpenAPI schema object code generation needs,
//! read permissively from raw JSON (3.0 and 3.1 spellings both).

use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub reference: Option<String>,
    /// `type`, as a list (3.1 allows several); `null` is moved to `nullable`.
    pub types: Vec<String>,
    pub format: Option<String>,
    pub nullable: bool,
    pub properties: Vec<(String, Schema)>,
    pub required: Vec<String>,
    pub additional: Additional,
    pub items: Option<Box<Schema>>,
    pub all_of: Vec<Schema>,
    pub one_of: Vec<Schema>,
    pub any_of: Vec<Schema>,
    pub enum_values: Vec<Value>,
    /// `x-enumNames` / `x-enum-varnames`.
    pub enum_names: Vec<String>,
    pub constant: Option<Value>,
    pub discriminator: Option<Discriminator>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub minimum: Option<f64>,
    /// `false`, `{"not": {}}`: matches nothing.
    pub never: bool,
}

#[derive(Debug, Clone, Default)]
pub enum Additional {
    /// No `additionalProperties`.
    #[default]
    Absent,
    /// `true` or `{}`.
    Any,
    /// `false`.
    Forbidden,
    Schema(Box<Schema>),
}

#[derive(Debug, Clone)]
pub struct Discriminator {
    pub property: String,
    /// Value → `$ref`.
    pub mapping: Vec<(String, String)>,
}

impl Schema {
    pub fn parse(value: &Value) -> Schema {
        let object = match value {
            Value::Object(object) => object,
            Value::Bool(false) => {
                return Schema {
                    never: true,
                    ..Default::default()
                }
            }
            _ => return Schema::default(),
        };
        let mut schema = Schema::default();
        let string = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);

        schema.reference = string("$ref");
        match object.get("type") {
            Some(Value::String(t)) => schema.types.push(t.clone()),
            Some(Value::Array(types)) => {
                schema.types = types
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
            }
            _ => {}
        }
        if schema.types.iter().any(|t| t == "null") {
            schema.nullable = true;
            schema.types.retain(|t| t != "null");
        }
        schema.format = string("format");
        if object.get("nullable").and_then(Value::as_bool) == Some(true)
            || object.get("x-nullable").and_then(Value::as_bool) == Some(true)
        {
            schema.nullable = true;
        }
        if let Some(Value::Object(properties)) = object.get("properties") {
            schema.properties = properties
                .iter()
                .map(|(name, s)| (name.clone(), Schema::parse(s)))
                .collect();
        }
        if let Some(Value::Array(required)) = object.get("required") {
            schema.required = required
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        schema.additional = match object.get("additionalProperties") {
            None => Additional::Absent,
            Some(Value::Bool(true)) => Additional::Any,
            Some(Value::Bool(false)) => Additional::Forbidden,
            Some(Value::Object(map)) if map.is_empty() => Additional::Any,
            Some(other) => Additional::Schema(Box::new(Schema::parse(other))),
        };
        schema.items = match object.get("items") {
            Some(Value::Array(tuple)) => tuple.first().map(|s| Box::new(Schema::parse(s))),
            Some(items) => Some(Box::new(Schema::parse(items))),
            None => None,
        };
        let list = |key: &str| -> Vec<Schema> {
            object
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.iter().map(Schema::parse).collect())
                .unwrap_or_default()
        };
        schema.all_of = list("allOf");
        schema.one_of = list("oneOf");
        schema.any_of = list("anyOf");
        if let Some(Value::Array(values)) = object.get("enum") {
            schema.enum_values = values.clone();
            if values.iter().any(Value::is_null) {
                schema.nullable = true;
                schema.enum_values.retain(|v| !v.is_null());
            }
        }
        for key in ["x-enumNames", "x-enum-varnames", "x-enum-names"] {
            if let Some(Value::Array(names)) = object.get(key) {
                schema.enum_names = names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                break;
            }
        }
        schema.constant = object.get("const").cloned();
        if let Some(Value::Object(discriminator)) = object.get("discriminator") {
            if let Some(property) = discriminator.get("propertyName").and_then(Value::as_str) {
                let mapping = discriminator
                    .get("mapping")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                            .collect()
                    })
                    .unwrap_or_default();
                schema.discriminator = Some(Discriminator {
                    property: property.to_owned(),
                    mapping,
                });
            }
        }
        schema.title = string("title");
        schema.description = string("description");
        schema.minimum = object.get("minimum").and_then(Value::as_f64);
        schema.deprecated = object
            .get("deprecated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(Value::Object(not)) = object.get("not") {
            schema.never = not.is_empty();
        }
        schema
    }

    /// An empty schema (`{}`): any value.
    pub fn is_any(&self) -> bool {
        self.reference.is_none()
            && self.types.is_empty()
            && self.properties.is_empty()
            && matches!(self.additional, Additional::Absent)
            && self.items.is_none()
            && self.all_of.is_empty()
            && self.one_of.is_empty()
            && self.any_of.is_empty()
            && self.enum_values.is_empty()
            && self.constant.is_none()
    }

    /// `{"type": "null"}`.
    pub fn is_null_only(&self) -> bool {
        self.nullable
            && self.types.is_empty()
            && self.reference.is_none()
            && self.properties.is_empty()
            && self.all_of.is_empty()
            && self.one_of.is_empty()
            && self.any_of.is_empty()
            && self.enum_values.is_empty()
            && self.items.is_none()
    }

    pub fn has_type(&self, name: &str) -> bool {
        self.types.iter().any(|t| t == name)
    }

    /// The description to document a field or type with.
    pub fn doc(&self) -> Option<String> {
        self.description
            .clone()
            .or_else(|| self.title.clone())
            .filter(|d| !d.trim().is_empty())
    }
}
