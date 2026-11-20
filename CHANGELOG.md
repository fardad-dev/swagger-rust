# Changelog

## 0.1.1

- `propertyNaming` (`snake_case`, `camelCase`): property names as the backend sends them when it renames the document's, as Spring with Jackson's `SNAKE_CASE` strategy does. Models send that spelling and decode it, falling back to the document's.

## 0.1.0 (unreleased)

First version: OpenAPI 3.0/3.1 and Swagger 2.0 input (JSON or YAML, url or file); serde models with strict or lenient decoding; async endpoint functions over a `Transport` trait; JSON, form, multipart, text and binary bodies; an editable `config.rs`; GPUI transport and cached queries (shared, retried, invalidated, prefetched, paged); swagger-typescript compatible names and config keys; `--check` for CI.
