//! `apis.rs`: one async function per endpoint.

use super::{doc_comment, generated_header, string_literal, Context, ALLOW};
use crate::ir::*;

pub fn render(_cx: &Context, operations: &[Operation]) -> String {
    let mut out = generated_header();
    out.push_str("//! One async function per endpoint. Pass any [`Transport`](runtime::Transport): with GPUI,\n");
    out.push_str("//! `cx.http_client()`; run the call on a background executor.\n");
    out.push_str(ALLOW);
    out.push_str("\nuse super::models;\nuse super::runtime;\n");
    for operation in operations {
        out.push('\n');
        out.push_str(&function(operation));
    }
    out
}

/// The documentation every generated wrapper of the endpoint shares.
pub fn operation_doc(operation: &Operation) -> String {
    let mut doc = format!("`{} {}`", operation.method.to_uppercase(), operation.path);
    if let Some(summary) = &operation.summary {
        doc.push_str("\n\n");
        doc.push_str(summary);
    }
    if let Some(description) = &operation.description {
        if operation.summary.as_deref() != Some(description.as_str()) {
            doc.push_str("\n\n");
            doc.push_str(description);
        }
    }
    let documented: Vec<&PathParam> = operation
        .path_params
        .iter()
        .filter(|p| p.doc.is_some())
        .collect();
    if !documented.is_empty() {
        doc.push_str("\n\n# Parameters\n");
        for param in documented {
            let text = param.doc.as_deref().unwrap_or_default().replace('\n', " ");
            doc.push_str(&format!("\n- `{}`: {}", param.name, text.trim()));
        }
    }
    doc
}

/// How a path parameter is passed to the `apis` function.
pub fn path_param_type(ty: &Ty) -> String {
    match ty {
        Ty::String => "&str".into(),
        Ty::Bool | Ty::I32 | Ty::I64 | Ty::F64 => ty.rust(Scope::Sibling),
        other => format!("&{}", other.rust(Scope::Sibling)),
    }
}

/// How the body is passed to the `apis` function.
pub fn body_param_type(body: &Body) -> String {
    match body {
        Body::Json { ty, .. } => match ty {
            Ty::String => "&str".into(),
            Ty::Vec(inner) => format!("&[{}]", inner.rust(Scope::Sibling)),
            other => format!("&{}", other.rust(Scope::Sibling)),
        },
        Body::Form { ty } => format!("&{}", ty.rust(Scope::Sibling)),
        Body::Multipart { type_name, .. } => format!("&models::{type_name}"),
        Body::Text { .. } => "&str".into(),
        Body::Bytes { .. } => "Vec<u8>".into(),
    }
}

pub fn response_type(response: &Response) -> String {
    match response {
        Response::Json(ty) => ty.rust(Scope::Sibling),
        Response::Text => "String".into(),
        Response::Bytes => "Vec<u8>".into(),
        Response::Empty => "()".into(),
    }
}

fn method_variant(method: &str) -> &'static str {
    match method {
        "get" => "Get",
        "put" => "Put",
        "post" => "Post",
        "delete" => "Delete",
        "options" => "Options",
        "head" => "Head",
        "patch" => "Patch",
        _ => "Trace",
    }
}

fn function(operation: &Operation) -> String {
    let mut out = doc_comment(&operation_doc(operation), "", false);
    if operation.deprecated {
        out.push_str("#[deprecated]\n");
    }
    out.push_str(&format!("pub async fn {}(\n", operation.fn_name));
    out.push_str("    transport: &(impl runtime::Transport + ?Sized),\n");
    for param in &operation.path_params {
        out.push_str(&format!(
            "    {}: {},\n",
            param.name,
            path_param_type(&param.ty)
        ));
    }
    if let Some(body) = &operation.body {
        out.push_str(&format!("    body: {},\n", body_param_type(body)));
    }
    if let Some(query) = &operation.query {
        out.push_str(&format!("    query: &models::{query},\n"));
    }
    if let Some(headers) = &operation.headers {
        out.push_str(&format!("    headers: &models::{headers},\n"));
    }
    out.push_str(&format!(
        ") -> Result<{}, runtime::ApiError> {{\n",
        response_type(&operation.response)
    ));

    if let Some(Body::Multipart { fields, .. }) = &operation.body {
        out.push_str("    let mut form = runtime::Multipart::new();\n");
        for field in fields {
            let name = string_literal(&field.json);
            let access = format!("body.{}", field.name);
            let call = |value: &str| match field.kind {
                MultipartKind::Text => format!("form.text({name}, {value});"),
                MultipartKind::Json => format!("form.json({name}, {value});"),
                MultipartKind::File => format!("form.file({name}, {value});"),
                MultipartKind::Files => {
                    format!("for file in {value} {{\n        form.file({name}, file);\n    }}")
                }
            };
            if field.optional {
                out.push_str(&format!(
                    "    if let Some(value) = &{access} {{\n        {}\n    }}\n",
                    call("value")
                ));
            } else {
                out.push_str(&format!("    {}\n", call(&format!("&{access}"))));
            }
        }
    }

    let path = if operation.path_params.is_empty() {
        string_literal(&operation.path)
    } else {
        let mut template = String::new();
        let mut args = Vec::new();
        let mut rest = operation.path.as_str();
        for param in &operation.path_params {
            let placeholder = format!("{{{}}}", param.json);
            if let Some(at) = rest.find(&placeholder) {
                template.push_str(&rest[..at].replace('{', "{{").replace('}', "}}"));
                template.push_str("{}");
                args.push(format!("runtime::path_param(&{})", param.name));
                rest = &rest[at + placeholder.len()..];
            }
        }
        template.push_str(&rest.replace('{', "{{").replace('}', "}}"));
        format!(
            "format!({}, {})",
            string_literal(&template),
            args.join(", ")
        )
    };
    out.push_str(&format!(
        "    let request = runtime::Request::new(runtime::Method::{}, {path})",
        method_variant(&operation.method)
    ));
    if operation.query.is_some() {
        out.push_str("\n        .query(query)");
    }
    if operation.headers.is_some() {
        out.push_str("\n        .headers(headers)");
    }
    match &operation.body {
        Some(Body::Json { content_type, .. }) => out.push_str(&format!(
            "\n        .json(&body, {})",
            string_literal(content_type)
        )),
        Some(Body::Form { .. }) => out.push_str("\n        .form(body)"),
        Some(Body::Multipart { .. }) => out.push_str("\n        .multipart(form)"),
        Some(Body::Text { content_type }) => out.push_str(&format!(
            "\n        .text(body, {})",
            string_literal(content_type)
        )),
        Some(Body::Bytes { content_type }) => out.push_str(&format!(
            "\n        .bytes(body, {})",
            string_literal(content_type)
        )),
        None => {}
    }
    let send = match &operation.response {
        Response::Json(_) => {
            out.push_str("\n        .accept(\"application/json\")");
            "send_json"
        }
        Response::Text => "send_text",
        Response::Bytes => "send_bytes",
        Response::Empty => "send_empty",
    };
    out.push_str(";\n");
    out.push_str(&format!(
        "    runtime::{send}(transport, request).await\n}}\n"
    ));
    out
}
