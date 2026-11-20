//! Paths → the endpoints to generate.

use crate::config::Config;
use crate::ir::*;
use crate::naming::{self, Names};
use crate::schema::Schema;
use crate::types::{Hint, Types};
use anyhow::Result;
use regex::Regex;
use serde_json::{Map, Value};

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Names the generated function's own parameters use.
const RESERVED_PARAMS: &[&str] = &[
    "transport",
    "query",
    "headers",
    "body",
    "request",
    "form",
    "cx",
    "http",
    "key",
    "page",
];

const PAGE_PARAMS: &[&str] = &["page", "pageno", "pagenumber", "offset"];

#[derive(Debug, Clone)]
struct Param {
    name: String,
    location: String,
    required: bool,
    schema: Schema,
    description: Option<String>,
}

pub fn build(
    document: &Value,
    config: &Config,
    types: &mut Types,
) -> Result<(Vec<Operation>, Vec<String>)> {
    let includes: Vec<Regex> = config
        .includes
        .iter()
        .map(|p| Regex::new(p))
        .collect::<Result<_, _>>()?;
    let excludes: Vec<Regex> = config
        .excludes
        .iter()
        .map(|p| Regex::new(p))
        .collect::<Result<_, _>>()?;
    let prefix = config
        .prefix
        .as_deref()
        .map(|p| Regex::new(&format!("(?i)^(?:{p})")))
        .transpose()?;
    let ignored_headers: Vec<String> = config
        .ignore
        .header_params
        .iter()
        .map(|h| h.to_lowercase())
        .chain(["accept", "content-type", "authorization"].map(str::to_owned))
        .collect();

    let mut warnings = Vec::new();
    let mut fn_names = Names::new();
    let mut operations = Vec::new();
    let empty = Map::new();
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        let path_params: Vec<Param> = parse_params(item.get("parameters"), types);
        for (method, operation) in item {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            let operation_id = operation.get("operationId").and_then(Value::as_str);
            let ts_name = naming::ts_service_name(
                config.method_name.as_deref(),
                prefix.as_ref(),
                method,
                path,
                operation_id,
            );
            let included = includes.is_empty() || includes.iter().any(|r| r.is_match(&ts_name));
            if !included || excludes.iter().any(|r| r.is_match(&ts_name)) {
                continue;
            }

            // Operation parameters override path-level ones with the same name and location.
            let own = parse_params(operation.get("parameters"), types);
            let mut params: Vec<Param> = path_params
                .iter()
                .filter(|p| {
                    !own.iter()
                        .any(|o| o.name == p.name && o.location == p.location)
                })
                .cloned()
                .collect();
            params.extend(own);

            let fn_name = fn_names.unique(&naming::value_ident(&ts_name));
            let prefix_name = naming::type_ident(&ts_name);
            let mut built = Operation {
                ts_name: ts_name.clone(),
                fn_name,
                method: method.clone(),
                path: path.clone(),
                summary: text(operation, "summary"),
                description: text(operation, "description"),
                deprecated: operation
                    .get("deprecated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                path_params: Vec::new(),
                query: None,
                headers: None,
                body: None,
                response: Response::Empty,
                query_kind: QueryKind::None,
            };

            let mut param_names = Names::with_reserved(RESERVED_PARAMS);
            for placeholder in placeholders(path) {
                let param = params
                    .iter()
                    .find(|p| p.location == "path" && p.name == placeholder);
                let (ty, doc) = match param {
                    Some(param) => {
                        let (ty, _) =
                            types.resolve(&param.schema, &Hint::field(&prefix_name, &param.name));
                        (ty, param.description.clone().or_else(|| param.schema.doc()))
                    }
                    None => {
                        warnings.push(format!(
                            "{} {path}: `{{{placeholder}}}` is not declared; taken as a string",
                            method.to_uppercase()
                        ));
                        (Ty::String, None)
                    }
                };
                built.path_params.push(PathParam {
                    json: placeholder.clone(),
                    name: param_names.unique(&naming::value_ident(&placeholder)),
                    ty,
                    doc,
                });
            }

            let query_params: Vec<&Param> =
                params.iter().filter(|p| p.location == "query").collect();
            let mut query_fields = Vec::new();
            if !query_params.is_empty() {
                let name = types.reserve_name(&format!("{prefix_name}QueryParams"));
                query_fields = param_fields(&name, &query_params, types);
                let mut fields = query_fields.clone();
                fields.push(extra_query_field(&query_fields));
                types.insert(Def::Struct(StructDef {
                    name: name.clone(),
                    doc: Some(format!(
                        "Query parameters of [`{}`](super::apis::{}).",
                        built.fn_name, built.fn_name
                    )),
                    fields,
                    input_only: true,
                    deprecated: false,
                }));
                built.query = Some(name);
            }

            let header_params: Vec<&Param> = params
                .iter()
                .filter(|p| {
                    p.location == "header" && !ignored_headers.contains(&p.name.to_lowercase())
                })
                .collect();
            if !header_params.is_empty() {
                let name = types.reserve_name(&format!("{prefix_name}HeaderParams"));
                let fields = param_fields(&name, &header_params, types);
                types.insert(Def::Struct(StructDef {
                    name: name.clone(),
                    doc: Some(format!(
                        "Header parameters of [`{}`](super::apis::{}).",
                        built.fn_name, built.fn_name
                    )),
                    fields,
                    input_only: true,
                    deprecated: false,
                }));
                built.headers = Some(name);
            }
            if params.iter().any(|p| p.location == "cookie") {
                warnings.push(format!(
                    "{} {path}: cookie parameters are not generated; set them in config.rs `prepare`",
                    method.to_uppercase()
                ));
            }

            built.body = request_body(operation, &prefix_name, types);
            built.response = response(operation, &prefix_name, types);

            if config.gpui {
                built.query_kind = query_kind(config, &built, &query_fields, &mut warnings);
            }
            operations.push(built);
        }
    }

    if config.gpui {
        for (key, list) in [
            ("useQuery", &config.use_query),
            ("useInfiniteQuery", &config.use_infinite_query),
        ] {
            for entry in list {
                let matched = operations
                    .iter()
                    .any(|op| listed(std::slice::from_ref(entry), op));
                if !matched {
                    warnings.push(format!(
                        "{key} entry `{entry}` matches no generated endpoint"
                    ));
                }
            }
        }
    }

    operations.sort_by(|a, b| a.fn_name.cmp(&b.fn_name));
    Ok((operations, warnings))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.trim().is_empty())
}

fn parse_params(value: Option<&Value>, types: &Types) -> Vec<Param> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let item = match item.get("$ref").and_then(Value::as_str) {
                Some(reference) => types.lookup(reference)?,
                None => item,
            };
            let name = item.get("name")?.as_str()?.to_owned();
            let location = item.get("in")?.as_str()?.to_owned();
            let mut schema = item.get("schema").map(Schema::parse).unwrap_or_else(|| {
                // OpenAPI 3 `content` parameters: take the first media type's schema.
                item.get("content")
                    .and_then(Value::as_object)
                    .and_then(|content| content.values().next())
                    .and_then(|media| media.get("schema"))
                    .map(Schema::parse)
                    .unwrap_or_default()
            });
            if schema.description.is_none() {
                schema.description = text(item, "description");
            }
            Some(Param {
                required: location == "path"
                    || item
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                description: text(item, "description"),
                name,
                location,
                schema,
            })
        })
        .collect()
}

/// The `extra` field of query parameter structs: parameters an API takes
/// but its document doesn't list (free-form filters, say).
fn extra_query_field(fields: &[Field]) -> Field {
    let taken = |name: &str| fields.iter().any(|f| f.name == name || f.json == name);
    let name = if taken("extra") {
        "extra_query"
    } else {
        "extra"
    };
    Field {
        json: name.to_owned(),
        name: name.to_owned(),
        ty: Ty::Object,
        optional: false,
        boxed: false,
        doc: Some(
            "Parameters the document doesn't list, sent like the others\n\
             (`price[min]=1`; arrays as `config::ARRAY_FORMAT` says)."
                .to_owned(),
        ),
        deprecated: false,
        flatten: true,
    }
}

fn param_fields(struct_name: &str, params: &[&Param], types: &mut Types) -> Vec<Field> {
    let mut names = Names::new();
    params
        .iter()
        .map(|param| {
            let (ty, nullable) =
                types.resolve(&param.schema, &Hint::field(struct_name, &param.name));
            Field {
                json: param.name.clone(),
                name: names.unique(&naming::value_ident(&param.name)),
                ty,
                optional: nullable || !param.required,
                boxed: false,
                doc: param.description.clone().or_else(|| param.schema.doc()),
                deprecated: param.schema.deprecated,
                flatten: false,
            }
        })
        .collect()
}

fn placeholders(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        out.push(after[..close].to_owned());
        rest = &after[close + 1..];
    }
    out
}

fn is_json(media_type: &str) -> bool {
    let media_type = media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    media_type == "*/*" || media_type.contains("json") || media_type == "application/*"
}

fn is_binary(schema: &Schema) -> bool {
    schema.has_type("string") && schema.format.as_deref() == Some("binary")
}

fn content<'a>(value: &'a Value, types: &'a Types) -> Option<&'a Map<String, Value>> {
    let value = match value.get("$ref").and_then(Value::as_str) {
        Some(reference) => types.lookup(reference)?,
        None => value,
    };
    value
        .get("content")
        .and_then(Value::as_object)
        .filter(|c| !c.is_empty())
}

fn request_body(operation: &Value, prefix: &str, types: &mut Types) -> Option<Body> {
    let body = operation.get("requestBody")?;
    let content = content(body, types)?.clone();
    let pick = |pred: &dyn Fn(&str) -> bool| content.keys().find(|k| pred(k.as_str())).cloned();
    let schema_of = |media_type: &str| {
        content
            .get(media_type)
            .and_then(|m| m.get("schema"))
            .map(Schema::parse)
    };

    if let Some(media_type) = pick(&|k| is_json(k)) {
        let schema = schema_of(&media_type).unwrap_or_default();
        if is_binary(&schema) {
            return Some(Body::Bytes {
                content_type: "application/octet-stream".into(),
            });
        }
        let (ty, _) = types.resolve(&schema, &Hint::exact(&format!("{prefix}RequestBody")));
        let content_type = if media_type.contains('*') {
            "application/json".to_owned()
        } else {
            media_type
        };
        return Some(Body::Json { ty, content_type });
    }
    if let Some(media_type) = pick(&|k| k.starts_with("multipart/")) {
        let schema = schema_of(&media_type).unwrap_or_default();
        return Some(multipart(prefix, &schema, types));
    }
    if let Some(media_type) = pick(&|k| k.starts_with("application/x-www-form-urlencoded")) {
        let schema = schema_of(&media_type).unwrap_or_default();
        let (ty, _) = types.resolve(&schema, &Hint::exact(&format!("{prefix}RequestBody")));
        return Some(Body::Form { ty });
    }
    if let Some(media_type) = pick(&|k| k.starts_with("text/")) {
        return Some(Body::Text {
            content_type: media_type,
        });
    }
    let media_type = content.keys().next()?.clone();
    Some(Body::Bytes {
        content_type: media_type,
    })
}

fn multipart(prefix: &str, schema: &Schema, types: &mut Types) -> Body {
    let (properties, required) = types.object_properties(schema);
    if properties.is_empty() {
        return Body::Bytes {
            content_type: "multipart/form-data".into(),
        };
    }
    let type_name = types.reserve_name(&format!("{prefix}RequestBody"));
    let mut names = Names::new();
    let mut fields = Vec::new();
    let mut parts = Vec::new();
    for (json, property) in properties {
        let resolved = match &property.reference {
            Some(reference) => types
                .lookup(reference)
                .map(Schema::parse)
                .unwrap_or_default(),
            None => property.clone(),
        };
        let name = names.unique(&naming::value_ident(&json));
        let (ty, nullable, kind) = if is_binary(&resolved) {
            (Ty::File, false, MultipartKind::File)
        } else if resolved.has_type("array") && resolved.items.as_deref().is_some_and(is_binary) {
            (Ty::Vec(Box::new(Ty::File)), false, MultipartKind::Files)
        } else {
            let (ty, nullable) = types.resolve(&property, &Hint::field(&type_name, &json));
            let kind = if is_scalar(&ty, types) {
                MultipartKind::Text
            } else {
                MultipartKind::Json
            };
            (ty, nullable, kind)
        };
        let optional = nullable || !required.contains(&json);
        parts.push(MultipartField {
            json: json.clone(),
            name: name.clone(),
            kind,
            optional,
        });
        fields.push(Field {
            json,
            name,
            ty,
            optional,
            boxed: false,
            doc: property.doc(),
            deprecated: property.deprecated,
            flatten: false,
        });
    }
    types.insert(Def::Struct(StructDef {
        name: type_name.clone(),
        doc: Some("A `multipart/form-data` request body.".into()),
        fields,
        input_only: true,
        deprecated: false,
    }));
    Body::Multipart {
        type_name,
        fields: parts,
    }
}

/// Sent as plain text in a form: strings, numbers, booleans and enums.
fn is_scalar(ty: &Ty, types: &Types) -> bool {
    match ty {
        Ty::String | Ty::Bool | Ty::I32 | Ty::I64 | Ty::F64 => true,
        Ty::Named(name) => match types.defs.get(name) {
            Some(Def::Enum(_)) => true,
            Some(Def::Alias(alias)) => is_scalar(&alias.ty, types),
            _ => false,
        },
        _ => false,
    }
}

fn response(operation: &Value, prefix: &str, types: &mut Types) -> Response {
    let Some(responses) = operation.get("responses").and_then(Value::as_object) else {
        return Response::Empty;
    };
    let preferred = [
        "200", "201", "202", "203", "204", "205", "206", "2XX", "2xx",
    ];
    let mut code = preferred
        .iter()
        .find(|c| responses.contains_key(**c))
        .map(|c| (*c).to_owned());
    if code.is_none() {
        let mut others: Vec<&String> = responses.keys().filter(|k| k.starts_with('2')).collect();
        others.sort();
        code = others.first().map(|c| (*c).clone());
    }
    if code.is_none() && responses.contains_key("default") {
        code = Some("default".into());
    }
    let Some(code) = code else {
        return Response::Empty;
    };
    let Some(content) = content(&responses[&code], types).cloned() else {
        return Response::Empty;
    };

    if let Some(media_type) = content.keys().find(|k| is_json(k)) {
        let Some(schema) = content[media_type].get("schema").map(Schema::parse) else {
            return Response::Json(Ty::Value);
        };
        if is_binary(&schema) {
            return Response::Bytes;
        }
        let (ty, _) = types.resolve(&schema, &Hint::exact(&format!("{prefix}Response")));
        return Response::Json(ty);
    }
    if content.keys().any(|k| k.starts_with("text/")) {
        return Response::Text;
    }
    Response::Bytes
}

fn listed(list: &[String], operation: &Operation) -> bool {
    let hook = format!("use{}", upper_first(&operation.ts_name));
    list.iter().any(|entry| {
        entry.eq_ignore_ascii_case(&operation.ts_name)
            || entry.eq_ignore_ascii_case(&hook)
            || *entry == operation.fn_name
    })
}

fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn query_kind(
    config: &Config,
    operation: &Operation,
    query_fields: &[Field],
    warnings: &mut Vec<String>,
) -> QueryKind {
    if listed(&config.use_infinite_query, operation) {
        let page = query_fields
            .iter()
            .find(|f| PAGE_PARAMS.contains(&f.json.to_lowercase().as_str()) && f.ty.is_integer());
        match page {
            Some(field) => {
                return QueryKind::Infinite {
                    page_field: field.name.clone(),
                    page_ty: field.ty.clone(),
                    optional: field.optional,
                }
            }
            None => warnings.push(format!(
                "`{}` is in useInfiniteQuery but has no integer page, pageNo, pageNumber or offset query parameter; generated as a plain query",
                operation.ts_name
            )),
        }
        return QueryKind::Query;
    }
    if operation.method == "get" || listed(&config.use_query, operation) {
        QueryKind::Query
    } else {
        QueryKind::None
    }
}
