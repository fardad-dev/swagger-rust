//! Generates typed Rust API clients from OpenAPI 3 / Swagger 2 documents.
//!
//! The output is plain Rust that depends only on `serde` and `serde_json`
//! (plus GPUI and `futures` for the GPUI layer): serde models, one async
//! function per endpoint over a small [`Transport`] trait, and optionally
//! GPUI queries with a shared cache, the counterpart of React Query.
//!
//! Use the `swagger-rust` binary, or call [`generate`] (from a build
//! script, say).
//!
//! [`Transport`]: https://docs.rs/swagger-rust/latest/swagger_rust/#transport

pub mod config;
mod emit;
mod ir;
pub mod load;
mod naming;
mod operations;
mod schema;
mod swagger2;
mod types;
mod write;

pub use config::{Config, Decode, PropertyNaming};

use anyhow::{Context as _, Result};
use std::path::PathBuf;

/// What [`generate`] did.
#[derive(Debug, Default)]
pub struct Report {
    pub endpoints: usize,
    pub types: usize,
    /// Files created or changed (with `check`: that would change).
    pub changed: Vec<PathBuf>,
    pub unchanged: usize,
    pub removed: Vec<PathBuf>,
    /// Parts of the document that could not be typed exactly.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Use the `swagger.json` saved in each output directory.
    pub local: bool,
    /// Write nothing; report what would change.
    pub check: bool,
}

/// Generates the client `config` describes into `config.dir`.
pub fn generate(config: &Config, options: Options) -> Result<Report> {
    let local = config.local || options.local;
    let document = if local {
        load::load_saved(&config.dir)?
    } else {
        load::load(&config.url)?
    };

    if config.keep_json && !local && !options.check {
        std::fs::create_dir_all(&config.dir)
            .with_context(|| format!("cannot create {}", config.dir.display()))?;
        let path = config.dir.join("swagger.json");
        let text = serde_json::to_string_pretty(&document)?;
        std::fs::write(&path, text + "\n")
            .with_context(|| format!("cannot write {}", path.display()))?;
    }

    generate_from_document(config, &document, options)
}

/// [`generate`] for a document already loaded (OpenAPI 3, see
/// [`load::normalize`]).
pub fn generate_from_document(
    config: &Config,
    document: &serde_json::Value,
    options: Options,
) -> Result<Report> {
    let mut types = types::Types::with_naming(document, config.property_naming);
    let (operations, mut warnings) = operations::build(document, config, &mut types)?;
    types.finish();
    warnings.extend(types.warnings.iter().cloned());

    let files = emit::render(config, document, &operations, &types);
    let written = write::write(&config.dir, files, config.rustfmt, options.check)?;
    Ok(Report {
        endpoints: operations.len(),
        types: types.defs.len(),
        changed: written.changed,
        unchanged: written.unchanged,
        removed: written.removed,
        warnings,
    })
}
