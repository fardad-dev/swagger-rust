//! Regenerates the clients in `tests/client` (compiled and tested there) and
//! `tests/gpui-client` from `tests/fixtures`, and fails if the committed
//! output is out of date. `UPDATE_SNAPSHOTS=1 cargo test` rewrites them.

use serde_json::json;
use std::path::{Path, PathBuf};
use swagger_rust::{generate, Config, Options};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn config(fixture: &str, dir: &Path, extra: serde_json::Value) -> Config {
    let mut value = json!({
        "url": root().join("tests/fixtures").join(fixture).to_string_lossy(),
        "dir": dir.to_string_lossy(),
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let config: Config = serde_json::from_value(value).unwrap();
    config.validate().unwrap();
    config
}

fn check(name: &str, config: Config) {
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let report = generate(
        &config,
        Options {
            local: false,
            check: !update,
        },
    )
    .unwrap();
    assert!(
        update || (report.changed.is_empty() && report.removed.is_empty()),
        "{name}: generated output differs from {} in {:?} (run `UPDATE_SNAPSHOTS=1 cargo test` and review the diff)",
        config.dir.display(),
        report.changed
    );
}

#[test]
fn petstore_strict() {
    let dir = root().join("tests/client/src/petstore_strict");
    check(
        "petstore_strict",
        config(
            "petstore.json",
            &dir,
            json!({ "decode": "strict", "ignore": { "headerParams": ["X-App-Version"] } }),
        ),
    );
}

#[test]
fn petstore_lenient() {
    let dir = root().join("tests/client/src/petstore_lenient");
    check(
        "petstore_lenient",
        config(
            "petstore.json",
            &dir,
            json!({ "decode": "lenient", "methodName": "{method}Store{path}" }),
        ),
    );
}

#[test]
fn swagger2() {
    let dir = root().join("tests/client/src/swagger2");
    check("swagger2", config("swagger2.json", &dir, json!({})));
}

#[test]
fn openapi31() {
    let dir = root().join("tests/client/src/openapi31");
    check(
        "openapi31",
        config("openapi31.yaml", &dir, json!({ "decode": "lenient" })),
    );
}

#[test]
fn naming_strict() {
    let dir = root().join("tests/client/src/naming_strict");
    check(
        "naming_strict",
        config(
            "naming.json",
            &dir,
            json!({ "propertyNaming": "snake_case" }),
        ),
    );
}

#[test]
fn naming_lenient() {
    let dir = root().join("tests/client/src/naming_lenient");
    check(
        "naming_lenient",
        config(
            "naming.json",
            &dir,
            json!({ "propertyNaming": "snake_case", "decode": "lenient" }),
        ),
    );
}

#[test]
fn petstore_gpui() {
    let dir = root().join("tests/gpui-client/src/petstore");
    check(
        "petstore_gpui",
        config(
            "petstore.json",
            &dir,
            json!({ "decode": "lenient", "gpui": true, "useInfiniteQuery": ["getFeed", "getFeedSpring"], "useQuery": ["postSearch"] }),
        ),
    );
}
