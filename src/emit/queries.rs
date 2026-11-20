//! `queries.rs`: the GPUI query of each endpoint that has one.

use super::apis::response_type;
use super::{doc_comment, generated_header, string_literal, Context, ALLOW};
use crate::ir::*;
use crate::naming;
use crate::types::Types;

pub fn render(cx: &Context, operations: &[Operation], types: &Types) -> String {
    let mut out = generated_header();
    out.push_str(
        "//! The endpoints as cached GPUI queries: see [`query_client`] for how they behave.\n",
    );
    out.push_str(ALLOW);
    out.push_str(&format!(
        "\nuse super::apis;\nuse super::models;\nuse super::query_client::{{self, InfiniteQuery, Query, QueryKey}};\nuse ::{}::{{App, Entity}};\n",
        cx.gpui()
    ));
    for operation in operations {
        if matches!(operation.query_kind, QueryKind::None) {
            continue;
        }
        out.push('\n');
        out.push_str(&query(operation, types));
    }
    out
}

/// An owned parameter of the query function, and how the `apis` call
/// borrows it.
struct Arg {
    name: String,
    ty: String,
    pass: String,
    copy: bool,
}

fn args(operation: &Operation) -> Vec<Arg> {
    let mut args = Vec::new();
    for param in &operation.path_params {
        let copy = matches!(param.ty, Ty::Bool | Ty::I32 | Ty::I64 | Ty::F64);
        let ty = param.ty.rust(Scope::Sibling);
        let pass = if copy {
            param.name.clone()
        } else {
            format!("&{}", param.name)
        };
        args.push(Arg {
            name: param.name.clone(),
            ty,
            pass,
            copy,
        });
    }
    if let Some(body) = &operation.body {
        let (ty, pass) = match body {
            Body::Json { ty, .. } | Body::Form { ty } => {
                (ty.rust(Scope::Sibling), "&body".to_owned())
            }
            Body::Multipart { type_name, .. } => {
                (format!("models::{type_name}"), "&body".to_owned())
            }
            Body::Text { .. } => ("String".to_owned(), "&body".to_owned()),
            Body::Bytes { .. } => ("Vec<u8>".to_owned(), "body.clone()".to_owned()),
        };
        args.push(Arg {
            name: "body".into(),
            ty,
            pass,
            copy: false,
        });
    }
    if let Some(query) = &operation.query {
        args.push(Arg {
            name: "query".into(),
            ty: format!("models::{query}"),
            pass: "&query".into(),
            copy: false,
        });
    }
    if let Some(headers) = &operation.headers {
        args.push(Arg {
            name: "headers".into(),
            ty: format!("models::{headers}"),
            pass: "&headers".into(),
            copy: false,
        });
    }
    args
}

fn query(operation: &Operation, types: &Types) -> String {
    let constant = naming::screaming(&operation.fn_name);
    let endpoint = format!("{} {}", operation.method.to_uppercase(), operation.path);
    let response = response_type(&operation.response);
    let args = args(operation);
    let signature: String = args
        .iter()
        .map(|a| format!(", {}: {}", a.name, a.ty))
        .collect();
    let key_parts: Vec<String> = args.iter().map(|a| format!("&{}", a.name)).collect();
    let key = if key_parts.is_empty() {
        format!("QueryKey::new({constant}, &())")
    } else {
        format!("QueryKey::new({constant}, &({},))", key_parts.join(", "))
    };
    let clones: String = args
        .iter()
        .filter(|a| !a.copy)
        .map(|a| format!("        let {0} = {0}.clone();\n", a.name))
        .collect();
    let call_args: String = args.iter().map(|a| format!(", {}", a.pass)).collect();
    let call = format!("apis::{}(&http{call_args}).await", operation.fn_name);
    let fn_name = &operation.fn_name;
    let deprecated = if operation.deprecated {
        "#[deprecated]\n"
    } else {
        ""
    };

    let mut out = format!(
        "/// The [`QueryKey`] endpoint of [`{fn_name}`], for [`query_client::invalidate_endpoint`].\npub const {constant}: &str = {};\n\n",
        string_literal(&endpoint)
    );
    out.push_str(&doc_comment(
        &super::apis::operation_doc(operation),
        "",
        false,
    ));

    let infinite = match &operation.query_kind {
        QueryKind::Infinite {
            page_field,
            page_ty,
            optional,
        } if serializable(&operation.response, types) => Some((page_field, page_ty, *optional)),
        _ => None,
    };
    match infinite {
        Some((page_field, page_ty, optional)) => {
            let page = if *page_ty == Ty::I32 {
                "page as i32"
            } else {
                "page"
            };
            let assign = if optional {
                format!("Some({page})")
            } else {
                page.to_owned()
            };
            out.push_str(&format!(
                "///\n/// Paged through `{page_field}`, from 1: see [`InfiniteQuery`].\n{deprecated}pub fn {fn_name}(cx: &mut App{signature}) -> Entity<InfiniteQuery<{response}>> {{\n"
            ));
            out.push_str(&format!("    let key = {key};\n"));
            out.push_str("    query_client::infinite_query(cx, key, move |http, page| {\n");
            out.push_str(&clones.replace(
                "let query = query.clone();",
                "let mut query = query.clone();",
            ));
            out.push_str(&format!("        query.{page_field} = {assign};\n"));
            out.push_str(&format!("        async move {{ {call} }}\n    }})\n}}\n"));
        }
        None => {
            out.push_str(&format!(
                "///\n/// A cached [`Query`]: see [`query_client`].\n{deprecated}pub fn {fn_name}(cx: &mut App{signature}) -> Entity<Query<{response}>> {{\n"
            ));
            out.push_str(&format!("    let key = {key};\n"));
            out.push_str("    query_client::query(cx, key, move |http| {\n");
            out.push_str(&clones);
            out.push_str(&format!("        async move {{ {call} }}\n    }})\n}}\n"));

            out.push_str(&format!(
                "\n/// Loads [`{fn_name}`] into the cache ahead of time.\n{deprecated}pub fn prefetch_{fn_name}(cx: &mut App{signature}) {{\n"
            ));
            out.push_str(&format!("    let key = {key};\n"));
            out.push_str("    query_client::prefetch(cx, key, move |http| {\n");
            out.push_str(&clones);
            out.push_str(&format!("        async move {{ {call} }}\n    }})\n}}\n"));
        }
    }
    out
}

/// Infinite queries read their pages as JSON, so the response must be a
/// JSON type.
fn serializable(response: &Response, _types: &Types) -> bool {
    matches!(response, Response::Json(_))
}
