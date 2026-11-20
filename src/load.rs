//! Reading the API document: from a url or a file, JSON or YAML, Swagger 2
//! converted to OpenAPI 3.

use crate::config::is_remote;
use crate::swagger2;
use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::{Map, Value};
use std::path::Path;

/// Loads the document at `url` (an `http(s)` url or a file path) as OpenAPI 3.
pub fn load(url: &str) -> Result<Value> {
    let text = if is_remote(url) {
        download(url)?
    } else {
        std::fs::read_to_string(url).with_context(|| format!("cannot read {url}"))?
    };
    let document = parse(&text).with_context(|| format!("{url} is neither JSON nor YAML"))?;
    normalize(document).with_context(|| format!("unsupported document at {url}"))
}

/// Reads a document saved by `keepJson`.
pub fn load_saved(dir: &Path) -> Result<Value> {
    let path = dir.join("swagger.json");
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "cannot read {} (`local` generation needs a document saved earlier with `keepJson`)",
            path.display()
        )
    })?;
    let document = parse(&text)?;
    normalize(document)
}

fn download(url: &str) -> Result<String> {
    let mut response = ureq::get(url)
        .header(
            "Accept",
            "application/json, application/yaml;q=0.9, */*;q=0.8",
        )
        .call()
        .map_err(|error| anyhow!("downloading {url} failed: {error}"))?;
    response
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_string()
        .map_err(|error| anyhow!("reading {url} failed: {error}"))
}

/// JSON, else YAML.
pub fn parse(text: &str) -> Result<Value> {
    let trimmed = text.trim_start_matches('\u{feff}');
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => Ok(value),
        Err(json_error) => {
            let yaml: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(trimmed).map_err(|yaml_error| {
                    anyhow!("not JSON ({json_error}) and not YAML ({yaml_error})")
                })?;
            yaml_to_json(yaml)
        }
    }
}

/// YAML allows keys JSON doesn't (`200:` is an integer key), so the
/// conversion is done by hand: every key becomes a string.
fn yaml_to_json(value: serde_yaml_ng::Value) -> Result<Value> {
    use serde_yaml_ng::Value as Yaml;
    Ok(match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(b) => Value::Bool(b),
        Yaml::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else {
                n.as_f64()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Yaml::String(s) => Value::String(s),
        Yaml::Sequence(items) => {
            Value::Array(items.into_iter().map(yaml_to_json).collect::<Result<_>>()?)
        }
        Yaml::Mapping(mapping) => {
            let mut map = Map::new();
            for (key, value) in mapping {
                let key = match key {
                    Yaml::String(s) => s,
                    Yaml::Number(n) => n.to_string(),
                    Yaml::Bool(b) => b.to_string(),
                    Yaml::Null => "null".to_owned(),
                    other => bail!("unsupported YAML key {other:?}"),
                };
                map.insert(key, yaml_to_json(value)?);
            }
            Value::Object(map)
        }
        Yaml::Tagged(tagged) => yaml_to_json(tagged.value)?,
    })
}

/// Swagger 2 is converted to OpenAPI 3; OpenAPI 3.x is used as it is.
pub fn normalize(document: Value) -> Result<Value> {
    if let Some(version) = document.get("swagger").and_then(Value::as_str) {
        if !version.starts_with('2') {
            bail!("Swagger version {version} is not supported (2.0 is)");
        }
        return Ok(swagger2::to_openapi3(document));
    }
    if let Some(version) = document.get("openapi").and_then(Value::as_str) {
        if !version.starts_with('3') {
            bail!("OpenAPI version {version} is not supported (3.0 and 3.1 are)");
        }
        return Ok(document);
    }
    bail!("the document has neither `openapi` nor `swagger` (is it an OpenAPI document?)")
}
