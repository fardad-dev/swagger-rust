//! `swagger-rust.json`: one entry per API to generate.
//!
//! The keys follow swagger-typescript's `swagger.config.json` (`url`, `dir`,
//! `methodName`, `prefix`, `includes`, `excludes`, `ignore.headerParams`,
//! `useQuery`, `useInfiniteQuery`, `keepJson`, `local`), so an existing entry
//! can be copied over; the Rust specific keys are documented on [`Config`].

use anyhow::{bail, Context as _, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The default config file name.
pub const DEFAULT_CONFIG_FILE: &str = "swagger-rust.json";

/// How generated models decode JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decode {
    /// Plain serde derives: a body that doesn't match the schema is an error.
    #[default]
    Strict,
    /// Tolerant decoding for loosely typed backends: numbers sent as strings
    /// (and back), `[]` for an empty object, unknown enum values and missing
    /// required fields fall back instead of failing the whole response.
    Lenient,
}

/// One API to generate.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// The OpenAPI 3 / Swagger 2 document: an `http(s)` url or a file path
    /// (relative to the config file), JSON or YAML.
    pub url: String,
    /// Output directory (relative to the config file). It becomes a Rust
    /// module: declare it with `mod <dir name>;` in its parent module.
    pub dir: PathBuf,
    /// Endpoint function names, from the placeholders `{method}`, `{path}`
    /// and `{operationId}`. Default `{method}{path}`, e.g. `GET /pets/{id}`
    /// gives `get_pets_id`.
    #[serde(default)]
    pub method_name: Option<String>,
    /// A path prefix (a regular expression, matched at the start) dropped
    /// from `{path}` before naming.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Only endpoints whose swagger-typescript style name (`getPetsId`)
    /// matches one of these regular expressions are generated.
    #[serde(default)]
    pub includes: Vec<String>,
    /// Endpoints whose name matches one of these are skipped.
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub ignore: Ignore,
    /// Non-GET endpoints that should still get a GPUI query (GET endpoints
    /// always do).
    #[serde(default)]
    pub use_query: Vec<String>,
    /// Endpoints whose GPUI query pages (`InfiniteQuery`) through a `page`,
    /// `pageNo`, `pageNumber` or `offset` query parameter.
    #[serde(default)]
    pub use_infinite_query: Vec<String>,
    /// Save the downloaded document as `swagger.json` in `dir`.
    #[serde(default)]
    pub keep_json: bool,
    /// Generate from the `swagger.json` saved in `dir` instead of `url`.
    #[serde(default)]
    pub local: bool,

    /// JSON decoding of the models: `strict` (default) or `lenient`.
    #[serde(default)]
    pub decode: Decode,
    /// Generate the GPUI layer: an `HttpClient` transport and cached queries.
    #[serde(default)]
    pub gpui: bool,
    /// The crate GPUI is used through (`gpui`, or a re-export such as
    /// `gpui_kit`).
    #[serde(default = "default_gpui_crate")]
    pub gpui_crate: String,
    /// Pass `HttpRequest::timeout` to GPUI's `HttpRequestExt::timeout`. Turn
    /// it off for GPUI versions whose http client has no `timeout`.
    #[serde(default = "default_true")]
    pub gpui_http_timeout: bool,
    /// The base url written into `config.rs` when it is first created.
    /// Defaults to the document's first absolute server url.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Format the output with `rustfmt` when it is installed.
    #[serde(default = "default_true")]
    pub rustfmt: bool,

    /// Accepted for compatibility with swagger-typescript configs; must be
    /// `rust` when present.
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default, rename = "$schema")]
    pub schema: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ignore {
    /// Header parameters left out of the generated functions (typically
    /// headers `config.rs` adds to every request).
    #[serde(default)]
    pub header_params: Vec<String>,
}

fn default_gpui_crate() -> String {
    "gpui".to_owned()
}

fn default_true() -> bool {
    true
}

impl Config {
    /// Resolves a relative `dir` and file `url` against `base` (the config
    /// file's directory).
    pub fn resolve_paths(&mut self, base: &Path) {
        let join = |path: &Path| -> PathBuf {
            let path = path.strip_prefix(".").unwrap_or(path);
            if base == Path::new(".") || base.as_os_str().is_empty() {
                path.to_path_buf()
            } else {
                base.join(path)
            }
        };
        if self.dir.is_relative() {
            self.dir = join(&self.dir);
        }
        if !is_remote(&self.url) && Path::new(&self.url).is_relative() {
            self.url = join(Path::new(&self.url)).to_string_lossy().into_owned();
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.url.trim().is_empty() {
            bail!("`url` is empty");
        }
        if self.dir.as_os_str().is_empty() {
            bail!("`dir` is empty");
        }
        if let Some(language) = &self.language {
            if language != "rust" {
                bail!("`language` is `{language}`; swagger-rust only generates `rust` (remove the key or set it to `rust`)");
            }
        }
        if self.gpui_crate.trim().is_empty() || self.gpui_crate.contains(char::is_whitespace) {
            bail!("`gpuiCrate` must be a crate path such as `gpui` or `gpui_kit`");
        }
        for pattern in self.includes.iter().chain(&self.excludes) {
            regex::Regex::new(pattern)
                .with_context(|| format!("invalid regular expression `{pattern}`"))?;
        }
        if let Some(prefix) = &self.prefix {
            regex::Regex::new(prefix).with_context(|| format!("invalid `prefix` `{prefix}`"))?;
        }
        Ok(())
    }

    /// The module name the output directory gives.
    pub fn module_name(&self) -> String {
        self.dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "api".to_owned())
    }
}

pub fn is_remote(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Reads a config file: a single entry or an array of entries. Relative
/// paths are resolved against the file's directory.
pub fn load(path: &Path) -> Result<Vec<Config>> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "cannot read {} (create one with `swagger-rust init`)",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let mut configs: Vec<Config> = match value {
        serde_json::Value::Array(_) => serde_json::from_value(value),
        _ => serde_json::from_value(value).map(|config| vec![config]),
    }
    .with_context(|| format!("invalid config in {}", path.display()))?;

    let base = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    for (index, config) in configs.iter_mut().enumerate() {
        config
            .validate()
            .with_context(|| format!("config entry {index} in {}", path.display()))?;
        config.resolve_paths(&base);
    }
    Ok(configs)
}

/// The starter file `swagger-rust init` writes.
pub const STARTER: &str = r#"[
  {
    "$schema": "https://raw.githubusercontent.com/fardad-dev/swagger-rust/main/schema/config.json",
    "url": "https://petstore3.swagger.io/api/v3/openapi.json",
    "dir": "./src/api/petstore",
    "methodName": "{method}{path}",
    "decode": "strict",
    "gpui": false
  }
]
"#;
