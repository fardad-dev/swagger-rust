//! Swagger 2.0 → OpenAPI 3.0, enough for code generation.
//!
//! * `definitions`, `parameters` and `responses` move under `components`, and
//!   every `$ref` is rewritten to match;
//! * `in: body` parameters become a `requestBody`, `in: formData` ones a form
//!   (`multipart/form-data` when `consumes` says so or a file is uploaded);
//! * the other parameters get their type under `schema`;
//! * a response `schema` goes under `content` with the operation's
//!   `produces` media type;
//! * `x-nullable` becomes `nullable`, `type: file` a binary string;
//! * `host`, `basePath` and `schemes` become `servers`.

use serde_json::{json, Map, Value};

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

pub fn to_openapi3(mut document: Value) -> Value {
    rewrite_refs(&mut document);
    let root = document.as_object().cloned().unwrap_or_default();

    let consumes = media_types(root.get("consumes"));
    let produces = media_types(root.get("produces"));

    let mut out = Map::new();
    out.insert("openapi".into(), json!("3.0.3"));
    out.insert(
        "info".into(),
        root.get("info")
            .cloned()
            .unwrap_or_else(|| json!({"title": "API", "version": ""})),
    );
    if let Some(server) = server_url(&root) {
        out.insert("servers".into(), json!([{ "url": server }]));
    }
    for key in ["tags", "security", "externalDocs"] {
        if let Some(value) = root.get(key) {
            out.insert(key.into(), value.clone());
        }
    }

    let global_parameters = root
        .get("parameters")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut paths = Map::new();
    if let Some(Value::Object(source_paths)) = root.get("paths") {
        for (path, item) in source_paths {
            let Some(item) = item.as_object() else {
                continue;
            };
            let mut new_item = Map::new();
            let path_parameters = item
                .get("parameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (key, value) in item {
                if METHODS.contains(&key.as_str()) {
                    let operation = convert_operation(
                        value,
                        &path_parameters,
                        &global_parameters,
                        &consumes,
                        &produces,
                    );
                    new_item.insert(key.clone(), operation);
                } else if key != "parameters" {
                    new_item.insert(key.clone(), value.clone());
                }
            }
            paths.insert(path.clone(), Value::Object(new_item));
        }
    }
    out.insert("paths".into(), Value::Object(paths));

    let mut components = Map::new();
    if let Some(Value::Object(definitions)) = root.get("definitions") {
        let schemas = definitions
            .iter()
            .map(|(name, schema)| (name.clone(), convert_schema(schema)))
            .collect();
        components.insert("schemas".into(), Value::Object(schemas));
    }
    let mut parameters = Map::new();
    let mut request_bodies = Map::new();
    for (name, parameter) in &global_parameters {
        match parameter.get("in").and_then(Value::as_str) {
            Some("body") => {
                let body = body_from_parameter(parameter, &consumes);
                request_bodies.insert(name.clone(), body);
            }
            Some("formData") => {}
            _ => {
                parameters.insert(name.clone(), convert_parameter(parameter));
            }
        }
    }
    if !parameters.is_empty() {
        components.insert("parameters".into(), Value::Object(parameters));
    }
    if !request_bodies.is_empty() {
        components.insert("requestBodies".into(), Value::Object(request_bodies));
    }
    if let Some(Value::Object(responses)) = root.get("responses") {
        let converted = responses
            .iter()
            .map(|(name, response)| (name.clone(), convert_response(response, &produces)))
            .collect();
        components.insert("responses".into(), Value::Object(converted));
    }
    if let Some(Value::Object(security)) = root.get("securityDefinitions") {
        components.insert("securitySchemes".into(), Value::Object(security.clone()));
    }
    out.insert("components".into(), Value::Object(components));
    Value::Object(out)
}

fn server_url(root: &Map<String, Value>) -> Option<String> {
    let host = root.get("host").and_then(Value::as_str)?;
    let base_path = root.get("basePath").and_then(Value::as_str).unwrap_or("");
    let schemes = media_types(root.get("schemes"));
    let scheme = if schemes.iter().any(|s| s == "https") || schemes.is_empty() {
        "https"
    } else {
        schemes[0].as_str()
    };
    Some(format!("{scheme}://{host}{base_path}"))
}

fn media_types(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn convert_operation(
    operation: &Value,
    path_parameters: &[Value],
    global_parameters: &Map<String, Value>,
    consumes: &[String],
    produces: &[String],
) -> Value {
    let Some(source) = operation.as_object() else {
        return operation.clone();
    };
    let consumes = match media_types(source.get("consumes")) {
        own if !own.is_empty() => own,
        _ => consumes.to_vec(),
    };
    let produces = match media_types(source.get("produces")) {
        own if !own.is_empty() => own,
        _ => produces.to_vec(),
    };

    // Operation parameters override path-level ones with the same name+in.
    let mut parameters: Vec<Value> = Vec::new();
    let resolve = |parameter: &Value| -> Value {
        match parameter.get("$ref").and_then(Value::as_str) {
            Some(reference) => {
                let name = reference.rsplit('/').next().unwrap_or_default();
                global_parameters
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| parameter.clone())
            }
            None => parameter.clone(),
        }
    };
    let own: Vec<Value> = source
        .get("parameters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let identity = |p: &Value| (p.get("name").cloned(), p.get("in").cloned());
    let own_resolved: Vec<Value> = own.iter().map(resolve).collect();
    for parameter in path_parameters.iter().map(resolve) {
        if !own_resolved
            .iter()
            .any(|p| identity(p) == identity(&parameter))
        {
            parameters.push(parameter);
        }
    }
    parameters.extend(own_resolved);

    let mut out = Map::new();
    for (key, value) in source {
        match key.as_str() {
            "parameters" | "consumes" | "produces" | "responses" | "schemes" => {}
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }

    let mut new_parameters = Vec::new();
    let mut form_fields: Vec<&Value> = Vec::new();
    for parameter in &parameters {
        match parameter.get("in").and_then(Value::as_str) {
            Some("body") => {
                out.insert(
                    "requestBody".into(),
                    body_from_parameter(parameter, &consumes),
                );
            }
            Some("formData") => form_fields.push(parameter),
            _ => new_parameters.push(convert_parameter(parameter)),
        }
    }
    if !form_fields.is_empty() {
        out.insert("requestBody".into(), form_body(&form_fields, &consumes));
    }
    if !new_parameters.is_empty() {
        out.insert("parameters".into(), Value::Array(new_parameters));
    }

    let mut responses = Map::new();
    if let Some(Value::Object(source_responses)) = source.get("responses") {
        for (code, response) in source_responses {
            responses.insert(code.clone(), convert_response(response, &produces));
        }
    }
    out.insert("responses".into(), Value::Object(responses));
    Value::Object(out)
}

fn body_from_parameter(parameter: &Value, consumes: &[String]) -> Value {
    let schema = parameter
        .get("schema")
        .map(convert_schema)
        .unwrap_or_else(|| json!({}));
    let media_type = consumes
        .iter()
        .find(|m| m.contains("json"))
        .or_else(|| consumes.first())
        .cloned()
        .unwrap_or_else(|| "application/json".to_owned());
    let mut body = Map::new();
    if let Some(description) = parameter.get("description") {
        body.insert("description".into(), description.clone());
    }
    body.insert(
        "required".into(),
        json!(parameter
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false)),
    );
    body.insert(
        "content".into(),
        json!({ media_type: { "schema": schema } }),
    );
    Value::Object(body)
}

fn form_body(fields: &[&Value], consumes: &[String]) -> Value {
    let has_file = fields
        .iter()
        .any(|f| f.get("type").and_then(Value::as_str) == Some("file"));
    let multipart = has_file || consumes.iter().any(|m| m == "multipart/form-data");
    let media_type = if multipart {
        "multipart/form-data"
    } else {
        "application/x-www-form-urlencoded"
    };
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        let Some(name) = field.get("name").and_then(Value::as_str) else {
            continue;
        };
        properties.insert(name.to_owned(), parameter_schema(field));
        if field.get("required").and_then(Value::as_bool) == Some(true) {
            required.push(json!(name));
        }
    }
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    json!({ "required": true, "content": { media_type: { "schema": schema } } })
}

fn convert_parameter(parameter: &Value) -> Value {
    let Some(source) = parameter.as_object() else {
        return parameter.clone();
    };
    if source.contains_key("$ref") {
        return parameter.clone();
    }
    let mut out = Map::new();
    for key in [
        "name",
        "in",
        "description",
        "required",
        "deprecated",
        "allowEmptyValue",
    ] {
        if let Some(value) = source.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    out.insert("schema".into(), parameter_schema(parameter));
    Value::Object(out)
}

/// The type keywords a Swagger 2 non-body parameter carries inline.
fn parameter_schema(parameter: &Value) -> Value {
    let Some(source) = parameter.as_object() else {
        return json!({});
    };
    if let Some(schema) = source.get("schema") {
        return convert_schema(schema);
    }
    let mut schema = Map::new();
    for key in [
        "type",
        "format",
        "items",
        "enum",
        "default",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "pattern",
        "minItems",
        "maxItems",
        "uniqueItems",
        "multipleOf",
        "x-nullable",
        "x-enumNames",
        "x-enum-varnames",
        "description",
    ] {
        if let Some(value) = source.get(key) {
            schema.insert(key.into(), value.clone());
        }
    }
    convert_schema(&Value::Object(schema))
}

fn convert_response(response: &Value, produces: &[String]) -> Value {
    let Some(source) = response.as_object() else {
        return response.clone();
    };
    if source.contains_key("$ref") {
        return response.clone();
    }
    let mut out = Map::new();
    out.insert(
        "description".into(),
        source
            .get("description")
            .cloned()
            .unwrap_or_else(|| json!("")),
    );
    if let Some(schema) = source.get("schema") {
        let media_type = produces
            .iter()
            .find(|m| m.contains("json"))
            .or_else(|| produces.first())
            .cloned()
            .unwrap_or_else(|| "application/json".to_owned());
        out.insert(
            "content".into(),
            json!({ media_type: { "schema": convert_schema(schema) } }),
        );
    }
    if let Some(headers) = source.get("headers") {
        out.insert("headers".into(), headers.clone());
    }
    Value::Object(out)
}

/// Swagger 2 schema keywords that differ from OpenAPI 3: `x-nullable` and
/// `type: file`. Applied recursively.
fn convert_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(source) => {
            let mut out = Map::new();
            for (key, value) in source {
                match key.as_str() {
                    "x-nullable" => {
                        out.insert("nullable".into(), value.clone());
                    }
                    "type" if value == "file" => {
                        out.insert("type".into(), json!("string"));
                        out.insert("format".into(), json!("binary"));
                    }
                    "properties" | "definitions" => {
                        let converted = value
                            .as_object()
                            .map(|map| {
                                map.iter()
                                    .map(|(k, v)| (k.clone(), convert_schema(v)))
                                    .collect()
                            })
                            .unwrap_or_default();
                        out.insert(key.clone(), Value::Object(converted));
                    }
                    "items" | "additionalProperties" | "not" => {
                        out.insert(key.clone(), convert_schema(value));
                    }
                    "allOf" | "oneOf" | "anyOf" => {
                        let converted = value
                            .as_array()
                            .map(|items| items.iter().map(convert_schema).collect())
                            .unwrap_or_default();
                        out.insert(key.clone(), Value::Array(converted));
                    }
                    _ => {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// `#/definitions/X` → `#/components/schemas/X`, and so on, everywhere.
fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "$ref" {
                    if let Value::String(reference) = child {
                        *reference = rewrite_ref(reference);
                    }
                } else {
                    rewrite_refs(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

fn rewrite_ref(reference: &str) -> String {
    for (from, to) in [
        ("#/definitions/", "#/components/schemas/"),
        ("#/parameters/", "#/components/parameters/"),
        ("#/responses/", "#/components/responses/"),
    ] {
        if let Some(rest) = reference.strip_prefix(from) {
            return format!("{to}{rest}");
        }
    }
    reference.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bodies_forms_refs_and_servers() {
        let swagger = json!({
            "swagger": "2.0",
            "host": "api.example.com",
            "basePath": "/v1",
            "schemes": ["http", "https"],
            "consumes": ["application/json"],
            "produces": ["application/json"],
            "paths": {
                "/pets/{id}": {
                    "parameters": [{"name": "id", "in": "path", "required": true, "type": "integer"}],
                    "put": {
                        "parameters": [
                            {"name": "body", "in": "body", "required": true, "schema": {"$ref": "#/definitions/pet.Pet"}},
                            {"name": "tags", "in": "query", "type": "array", "items": {"type": "string"}}
                        ],
                        "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/pet.Pet"}}}
                    },
                    "post": {
                        "consumes": ["multipart/form-data"],
                        "parameters": [{"name": "file", "in": "formData", "type": "file", "required": true}],
                        "responses": {"204": {"description": "done"}}
                    }
                }
            },
            "definitions": {
                "pet.Pet": {"type": "object", "properties": {"name": {"type": "string", "x-nullable": true}}}
            }
        });
        let openapi = to_openapi3(swagger);
        assert_eq!(openapi["servers"][0]["url"], "https://api.example.com/v1");
        let put = &openapi["paths"]["/pets/{id}"]["put"];
        assert_eq!(
            put["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/pet.Pet"
        );
        assert_eq!(put["parameters"][0]["name"], "id");
        assert_eq!(put["parameters"][0]["schema"]["type"], "integer");
        assert_eq!(put["parameters"][1]["schema"]["items"]["type"], "string");
        assert_eq!(
            put["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/pet.Pet"
        );
        let post = &openapi["paths"]["/pets/{id}"]["post"];
        let file =
            &post["requestBody"]["content"]["multipart/form-data"]["schema"]["properties"]["file"];
        assert_eq!(file["format"], "binary");
        assert_eq!(
            openapi["components"]["schemas"]["pet.Pet"]["properties"]["name"]["nullable"],
            true
        );
    }
}
