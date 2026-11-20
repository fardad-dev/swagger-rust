//! Names: swagger-typescript's endpoint names (so `includes`, `excludes`
//! and `useInfiniteQuery` entries keep matching) and their Rust spellings.

use std::collections::HashSet;

/// Rust keywords (strict, reserved and edition specific). They get a
/// trailing `_` when used as names.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "try", "type",
    "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Type names the generated code uses unqualified; models with these names
/// get a `Model` suffix.
pub const RESERVED_TYPES: &[&str] = &[
    "Arc",
    "BTreeMap",
    "Box",
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Deserialize",
    "Deserializer",
    "Display",
    "Eq",
    "Err",
    "FromJson",
    "HashMap",
    "Hash",
    "Map",
    "None",
    "Ok",
    "Option",
    "PartialEq",
    "Result",
    "Self",
    "Serialize",
    "Serializer",
    "Some",
    "String",
    "ToString",
    "Value",
    "Vec",
    "Into",
    "From",
    "Iterator",
    "Send",
    "Sync",
    "Sized",
    "FilePart",
    "Multipart",
];

/// Splits an identifier into words: at non-alphanumeric characters, at
/// lower→upper case changes (`petId`), before the last capital of an acronym
/// (`HTTPServer` → `HTTP`, `Server`) and at digit→upper changes (`v2Api`).
pub fn words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    for chunk in input.split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = chunk.chars().collect();
        let mut current = String::new();
        for (i, &c) in chars.iter().enumerate() {
            if !current.is_empty() && c.is_uppercase() {
                let prev = chars[i - 1];
                let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
                if prev.is_lowercase()
                    || prev.is_numeric()
                    || (prev.is_uppercase() && next_is_lower)
                {
                    words.push(std::mem::take(&mut current));
                }
            }
            current.push(c);
        }
        if !current.is_empty() {
            words.push(current);
        }
    }
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// `pet_store.PetDTO` → `PetStorePetDto`.
pub fn pascal(input: &str) -> String {
    words(input).iter().map(|w| capitalize(w)).collect()
}

/// `getPetsPetId` → `get_pets_pet_id`.
pub fn snake(input: &str) -> String {
    words(input)
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// Jackson's `SnakeCaseStrategy`, exactly: an `_` before an upper-case
/// letter that follows a non-upper-case one, then lower case
/// (`titleFa` → `title_fa`, `productID` → `product_id`, `URLPath` → `urlpath`).
pub fn jackson_snake(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    let mut previous_upper = false;
    for (index, c) in input.chars().enumerate() {
        if index == 0 && c == '_' {
            continue;
        }
        if c.is_uppercase() {
            if !previous_upper && !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
            previous_upper = true;
        } else {
            out.push(c);
            previous_upper = false;
        }
    }
    out
}

/// `title_fa` → `titleFa`; names without `_` are kept.
pub fn lower_camel(input: &str) -> String {
    if !input.contains('_') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut upper_next = false;
    for c in input.chars() {
        if c == '_' {
            upper_next = !out.is_empty();
        } else if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `getPetsPetId` → `GET_PETS_PET_ID`.
pub fn screaming(input: &str) -> String {
    snake(input).to_uppercase()
}

/// A field, function or parameter name.
pub fn value_ident(input: &str) -> String {
    let mut name = snake(&ascii_only(input));
    if name.is_empty() {
        name = "value".to_owned();
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    if KEYWORDS.contains(&name.as_str()) {
        name.push('_');
    }
    name
}

/// A type name.
pub fn type_ident(input: &str) -> String {
    let mut name = pascal(&ascii_only(input));
    if name.is_empty() {
        name = "Model".to_owned();
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, 'T');
    }
    if RESERVED_TYPES.contains(&name.as_str()) || KEYWORDS.contains(&name.as_str()) {
        name.push_str("Model");
    }
    name
}

/// An enum variant name.
pub fn variant_ident(input: &str) -> String {
    let mut name = pascal(&ascii_only(input));
    if name.is_empty() {
        name = "Empty".to_owned();
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, 'V');
    }
    if name == "Self" {
        name.push('_');
    }
    name
}

/// Non-ASCII letters (Persian enum values, say) are spelled by code point,
/// so names stay valid and distinct.
fn ascii_only(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        if c.is_ascii() {
            out.push(c);
        } else if c.is_alphanumeric() {
            out.push_str(&format!("_u{:x}_", c as u32));
        } else {
            out.push('_');
        }
    }
    out
}

/// swagger-typescript's `getSchemaName`: every `/ . \` [ ] - * { }` drops
/// out and the character after it is upper-cased.
pub fn ts_schema_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = true;
    for c in name.chars() {
        if matches!(c, '/' | '.' | '`' | '[' | ']' | '-' | '*' | '{' | '}') {
            upper_next = true;
        } else if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// swagger-typescript's service name: `methodName` (default
/// `{method}{path}`) with `{method}`, `{path}` and `{operationId}` filled in.
/// A template using `{operationId}` falls back to the default for
/// operations without one.
pub fn ts_service_name(
    template: Option<&str>,
    prefix: Option<&regex::Regex>,
    method: &str,
    path: &str,
    operation_id: Option<&str>,
) -> String {
    let default = "{method}{path}";
    let template = match template {
        Some(t) if t.to_lowercase().contains("{operationid}") && operation_id.is_none() => default,
        Some(t) => t,
        None => default,
    };
    let path = match prefix {
        Some(prefix) => match prefix.find(path) {
            Some(found) if found.start() == 0 => &path[found.end()..],
            _ => path,
        },
        None => path,
    };
    let path = ts_schema_name(path);
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                match key.to_lowercase().as_str() {
                    "method" => out.push_str(method),
                    "path" => out.push_str(&path),
                    "operationid" => out.push_str(operation_id.unwrap_or_default()),
                    _ => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Hands out names that are unique within one scope.
#[derive(Debug, Default, Clone)]
pub struct Names {
    taken: HashSet<String>,
}

impl Names {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_reserved(reserved: &[&str]) -> Self {
        Self {
            taken: reserved.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    pub fn is_taken(&self, name: &str) -> bool {
        self.taken.contains(name)
    }

    pub fn reserve(&mut self, name: &str) {
        self.taken.insert(name.to_owned());
    }

    /// `name`, or `name2`, `name3`… (`name_2` for snake case names).
    pub fn unique(&mut self, name: &str) -> String {
        if self.taken.insert(name.to_owned()) {
            return name.to_owned();
        }
        // `get_pet_2`, `Pet2`, but `Variant1_2` (not `Variant12`).
        let separator = if name.contains('_')
            || name.chars().all(|c| !c.is_uppercase())
            || name.ends_with(|c: char| c.is_ascii_digit())
        {
            "_"
        } else {
            ""
        };
        let base = name.trim_end_matches('_');
        (2..)
            .map(|n| format!("{base}{separator}{n}"))
            .find(|candidate| self.taken.insert(candidate.clone()))
            .expect("an unused name")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words() {
        assert_eq!(
            words("getStoreV2Autocomplete"),
            ["get", "Store", "V2", "Autocomplete"]
        );
        assert_eq!(words("HTTPServerError"), ["HTTP", "Server", "Error"]);
        assert_eq!(
            words("early_access_offers.WidgetData"),
            ["early", "access", "offers", "Widget", "Data"]
        );
        assert_eq!(words("x-site"), ["x", "site"]);
    }

    #[test]
    fn cases() {
        assert_eq!(
            snake("getStoreBrandsBrand_identifierProducts"),
            "get_store_brands_brand_identifier_products"
        );
        assert_eq!(pascal("product_list.LayoutView"), "ProductListLayoutView");
        assert_eq!(screaming("getPetsId"), "GET_PETS_ID");
        assert_eq!(value_ident("type"), "type_");
        assert_eq!(value_ident("3d"), "_3d");
        assert_eq!(value_ident("X-City-ID"), "x_city_id");
        assert_eq!(type_ident("Option"), "OptionModel");
        assert_eq!(variant_ident("ORG_EXCLUSIVE"), "OrgExclusive");
        assert_eq!(variant_ident("2xl"), "V2xl");
        assert_eq!(variant_ident(""), "Empty");
    }

    #[test]
    fn wire_names() {
        assert_eq!(jackson_snake("titleFa"), "title_fa");
        assert_eq!(jackson_snake("isInactive"), "is_inactive");
        assert_eq!(jackson_snake("productID"), "product_id");
        assert_eq!(jackson_snake("URLPath"), "urlpath");
        assert_eq!(jackson_snake("categoryLevel1"), "category_level1");
        assert_eq!(jackson_snake("already_snake"), "already_snake");
        assert_eq!(lower_camel("title_fa"), "titleFa");
        assert_eq!(lower_camel("id"), "id");
    }

    #[test]
    fn typescript_service_names() {
        assert_eq!(
            ts_schema_name("/brands/{brand_identifier}/products"),
            "BrandsBrand_identifierProducts"
        );
        assert_eq!(
            ts_service_name(Some("{method}Store{path}"), None, "get", "/home", None),
            "getStoreHome"
        );
        assert_eq!(
            ts_service_name(None, None, "post", "/v2/pets", None),
            "postV2Pets"
        );
        let prefix = regex::Regex::new("/api").unwrap();
        assert_eq!(
            ts_service_name(None, Some(&prefix), "get", "/api/pets", None),
            "getPets"
        );
        assert_eq!(
            ts_service_name(Some("{operationId}"), None, "get", "/pets", None),
            "getPets"
        );
        assert_eq!(
            ts_service_name(
                Some("{operationId}"),
                None,
                "get",
                "/pets",
                Some("listPets")
            ),
            "listPets"
        );
    }

    #[test]
    fn unique_names() {
        let mut names = Names::new();
        assert_eq!(names.unique("Pet"), "Pet");
        assert_eq!(names.unique("Pet"), "Pet2");
        assert_eq!(names.unique("get_pet"), "get_pet");
        assert_eq!(names.unique("get_pet"), "get_pet_2");
    }
}
