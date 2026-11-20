//! The generated clients against a mock transport: what goes on the wire
//! and how responses decode, in both decoding modes.

use futures::executor::block_on;
use serde_json::json;
use std::sync::Mutex;
use swagger_rust_test_client::{
    naming_lenient, naming_strict, openapi31, petstore_lenient as lenient, petstore_strict as strict, swagger2,
};

#[derive(Debug, Clone)]
struct Sent {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Sent {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Answers every request with one canned response and records the requests.
struct Mock {
    status: u16,
    body: Vec<u8>,
    sent: Mutex<Vec<Sent>>,
}

impl Mock {
    fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self { status, body: body.into(), sent: Mutex::new(Vec::new()) }
    }

    fn json(value: serde_json::Value) -> Self {
        Self::new(200, value.to_string())
    }

    fn last(&self) -> Sent {
        self.sent.lock().unwrap().last().cloned().expect("a request was sent")
    }
}

macro_rules! mock_transport {
    ($($client:ident),*) => {$(
        impl $client::runtime::Transport for Mock {
            fn send(
                &self,
                request: $client::runtime::HttpRequest,
            ) -> $client::runtime::BoxFuture<'static, Result<$client::runtime::HttpResponse, $client::runtime::TransportError>> {
                self.sent.lock().unwrap().push(Sent {
                    method: request.method.as_str().to_owned(),
                    url: request.url,
                    headers: request.headers,
                    body: request.body,
                });
                let response = $client::runtime::HttpResponse { status: self.status, headers: Vec::new(), body: self.body.clone() };
                Box::pin(async move { Ok(response) })
            }
        }
    )*};
}
mock_transport!(strict, lenient, swagger2, openapi31, naming_strict, naming_lenient);

#[test]
fn query_and_header_parameters_and_typed_response() {
    use strict::models::{GetPetsHeaderParams, GetPetsQueryParams, PetStatus, Size};
    let mock = Mock::json(json!([{
        "id": 1, "name": "Rex", "status": "available", "type": "dog", "3d_model": "rex.glb",
        "tags": [{"id": 1, "name": "good"}], "size": 2,
        "parent": {"id": 0, "name": "Mom", "status": "sold"}
    }]));
    let extra = json!({"price": {"min": 1}, "brands": [18, 3]});
    let query = GetPetsQueryParams {
        status: Some(vec![PetStatus::Available, PetStatus::Sold]),
        limit: Some(10),
        extra: extra.as_object().unwrap().clone(),
    };
    let headers = GetPetsHeaderParams { x_request_id: "abc".into() };
    let pets = block_on(strict::apis::get_pets(&mock, &query, &headers)).unwrap();

    let sent = mock.last();
    assert_eq!(sent.method, "GET");
    assert_eq!(
        sent.url,
        "https://eu.petstore.example/v1/pets?status=available&status=sold&limit=10&price%5Bmin%5D=1&brands=18&brands=3"
    );
    assert_eq!(sent.header("X-Request-Id"), Some("abc"));
    assert_eq!(sent.header("Accept"), Some("application/json"));
    assert!(sent.header("X-App-Version").is_none(), "ignored header parameter");

    let rex = &pets[0];
    assert_eq!((rex.id, rex.name.as_str(), &rex.status), (1, "Rex", &PetStatus::Available));
    assert_eq!(rex.type_.as_deref(), Some("dog"));
    assert_eq!(rex._3d_model.as_deref(), Some("rex.glb"));
    assert_eq!(rex.size, Some(Size::Medium));
    assert_eq!(rex.parent.as_ref().unwrap().name, "Mom");
    assert_eq!(rex.tags.as_ref().unwrap()[0].name.as_deref(), Some("good"));
}

#[test]
fn strict_decoding_rejects_a_mistyped_body() {
    let mock = Mock::json(json!({"id": "1", "name": "Rex", "status": "available"}));
    let error = block_on(strict::apis::get_pets_pet_id(&mock, 1)).unwrap_err();
    assert!(matches!(error, strict::ApiError::Decode { .. }), "{error}");
}

#[test]
fn lenient_decoding_survives_a_drifting_backend() {
    use lenient::models::{PetStatus, Size};
    let mock = Mock::json(json!({
        "id": "42", "name": null, "status": "lost", "tags": {}, "attributes": [],
        "weight": "3.5", "vaccinated": 1, "size": "9", "owner": [], "children": [{"id": 7}, "garbage"]
    }));
    let pet = block_on(lenient::apis::get_store_pets_pet_id(&mock, 42)).unwrap();
    assert_eq!(pet.id, 42);
    assert_eq!(pet.name, "", "a null required field falls back to its default");
    assert_eq!(pet.status, PetStatus::Unknown("lost".into()));
    assert_eq!(pet.tags, Some(vec![]));
    assert_eq!(pet.attributes, Some(Default::default()));
    assert_eq!(pet.weight, Some(3.5));
    assert_eq!(pet.vaccinated, Some(true));
    assert_eq!(pet.size, Some(Size::Unknown(9)));
    assert_eq!(pet.owner, Some(Default::default()));
    let children = pet.children.unwrap();
    assert_eq!(children.len(), 1, "an unreadable element is skipped");
    assert_eq!(children[0].id, 7);

    // The models are serde types too.
    let parsed: lenient::models::Pet = serde_json::from_str(r#"{"id": "7", "status": "sold"}"#).unwrap();
    assert_eq!((parsed.id, parsed.status), (7, PetStatus::Sold));
}

#[test]
fn path_parameters_are_encoded() {
    use strict::models::{GetOwnersOwnerNamePetsQueryParams, Kind};
    let mock = Mock::json(json!([]));
    let query = GetOwnersOwnerNamePetsQueryParams { kind: Some(Kind::Cat), ..Default::default() };
    block_on(strict::apis::get_owners_owner_name_pets(&mock, "Ann Lee/2", &query)).unwrap();
    assert_eq!(mock.last().url, "https://eu.petstore.example/v1/owners/Ann%20Lee%2F2/pets?kind=cat");
}

#[test]
fn http_errors_carry_status_and_message() {
    let mock = Mock::new(404, r#"{"message": "no such pet"}"#);
    let error = block_on(strict::apis::get_pets_pet_id(&mock, 9)).unwrap_err();
    assert_eq!(error.status(), Some(404));
    assert_eq!(error.message(), Some("no such pet"));
    assert!(!error.is_retryable());
    let error = block_on(strict::apis::get_pets_pet_id(&Mock::new(503, ""), 9)).unwrap_err();
    assert!(error.is_retryable());
}

#[test]
fn multipart_bodies() {
    use strict::models::{Meta, PetStatus, PutPetsPetIdPhotoRequestBody};
    use strict::runtime::FilePart;
    let mock = Mock::json(json!({"url": "https://cdn/p.png"}));
    let body = PutPetsPetIdPhotoRequestBody {
        file: FilePart::new("a.png", "image/png", b"PNG".to_vec()),
        extra: Some(vec![FilePart::new("b.png", "", b"B".to_vec())]),
        caption: Some("hi".into()),
        status: Some(PetStatus::Pending),
        meta: Some(Meta { width: Some(3) }),
    };
    let response = block_on(strict::apis::put_pets_pet_id_photo(&mock, 5, &body)).unwrap();
    assert_eq!(response.url.as_deref(), Some("https://cdn/p.png"));

    let sent = mock.last();
    let content_type = sent.header("Content-Type").unwrap().to_owned();
    let boundary = content_type.strip_prefix("multipart/form-data; boundary=").expect("multipart");
    let text = sent.body_text();
    assert!(text.contains("name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\nPNG\r\n"));
    assert!(text.contains("name=\"extra\"; filename=\"b.png\"\r\nContent-Type: application/octet-stream"));
    assert!(text.contains("name=\"caption\"\r\n\r\nhi\r\n"));
    assert!(text.contains("name=\"status\"\r\n\r\npending\r\n"));
    assert!(text.contains("name=\"meta\"\r\nContent-Type: application/json\r\n\r\n{\"width\":3}\r\n"));
    assert!(text.ends_with(&format!("--{boundary}--\r\n")));
}

#[test]
fn form_bodies_text_bytes_and_empty_responses() {
    use strict::models::PostLoginRequestBody;
    let mock = Mock::new(200, "token-1");
    let body = PostLoginRequestBody { user: "ann lee".into(), scopes: Some(vec!["a".into(), "b".into()]) };
    let token = block_on(strict::apis::post_login(&mock, &body)).unwrap();
    assert_eq!(token, "token-1");
    let sent = mock.last();
    assert_eq!(sent.header("Content-Type"), Some("application/x-www-form-urlencoded"));
    assert_eq!(sent.body_text(), "user=ann%20lee&scopes=a&scopes=b");

    let image = block_on(strict::apis::get_pets_pet_id_image(&Mock::new(200, vec![1u8, 2, 3]), 1)).unwrap();
    assert_eq!(image, vec![1, 2, 3]);

    #[allow(deprecated)]
    let deleted = block_on(strict::apis::delete_pets_pet_id(&Mock::new(204, ""), 1));
    assert!(deleted.is_ok());
}

#[test]
fn unions_by_discriminator_and_by_shape() {
    use strict::models::{Animal, Shape, StringOrNumber};
    let dog = block_on(strict::apis::get_animals_id(&Mock::json(json!({"kind": "dog", "good": true})), "d")).unwrap();
    assert!(matches!(dog, Animal::Dog(ref d) if d.good == Some(true)));
    let bird = block_on(strict::apis::get_animals_id(&Mock::json(json!({"kind": "bird"})), "b")).unwrap();
    assert!(matches!(bird, Animal::Unknown(_)));

    let shapes = block_on(strict::apis::get_shapes(&Mock::json(json!([
        {"side": 2}, {"radius": 1}, {"side": 1, "rounded": true}
    ]))))
    .unwrap();
    assert!(matches!(shapes[0], Shape::Square(_)));
    assert!(matches!(shapes[1], Shape::Circle(_)));
    assert!(matches!(shapes[2], Shape::Square(ref s) if s.rounded == Some(true)));

    let values = block_on(lenient::apis::post_store_search(
        &Mock::json(json!(["a", 3, true])),
        &lenient::models::PostStoreSearchRequestBody::default(),
    ))
    .unwrap();
    assert_eq!(values.len(), 3);
    assert!(matches!(values[0], lenient::models::StringOrNumber::String(ref s) if s == "a"));
    assert!(matches!(values[1], lenient::models::StringOrNumber::Integer(3)));
    assert!(matches!(values[2], lenient::models::StringOrNumber::Unknown(_)));
    // The strict client's variants round-trip through serde.
    let back = serde_json::to_value(StringOrNumber::Integer(4)).unwrap();
    assert_eq!(back, json!(4));
}

#[test]
fn enums_convert_and_serialize() {
    use strict::models::PetStatus;
    assert_eq!(PetStatus::from("sold"), PetStatus::Sold);
    assert_eq!(PetStatus::Unknown("new".into()).to_string(), "new");
    assert_eq!(serde_json::to_string(&PetStatus::Pending).unwrap(), "\"pending\"");
    assert_eq!(serde_json::from_str::<PetStatus>("\"lost\"").unwrap(), PetStatus::Unknown("lost".into()));
}

#[test]
fn query_string_formats() {
    use strict::runtime::{encode_pairs, query_pairs, ArrayFormat};
    let value = json!({"ids": [1, 2], "filter": {"color": "red"}, "q": "a b", "none": null});
    let encode = |format| encode_pairs(&query_pairs(&value, format));
    assert_eq!(encode(ArrayFormat::Repeat), "ids=1&ids=2&filter%5Bcolor%5D=red&q=a%20b");
    assert_eq!(encode(ArrayFormat::Indices), "ids%5B0%5D=1&ids%5B1%5D=2&filter%5Bcolor%5D=red&q=a%20b");
    assert_eq!(encode(ArrayFormat::Brackets), "ids%5B%5D=1&ids%5B%5D=2&filter%5Bcolor%5D=red&q=a%20b");
    assert_eq!(encode(ArrayFormat::Comma), "ids=1%2C2&filter%5Bcolor%5D=red&q=a%20b");
}

#[test]
fn swagger2_documents() {
    use swagger2::models::{GetOrdersIdQueryParams, OrderOrder, OrderState};
    let mock = Mock::json(json!({"id": 1, "state": 2, "note": null, "shipping": {"city": "Tehran"}, "lines": [{"sku": "x", "qty": 2}]}));
    let query = GetOrdersIdQueryParams { expand: Some(vec!["lines".into()]), ..Default::default() };
    let order = block_on(swagger2::apis::get_orders_id(&mock, 1, &query)).unwrap();
    assert_eq!(mock.last().url, "https://store.example/api/orders/1?expand=lines");
    assert_eq!(order.state, OrderState::StateShipped);
    assert_eq!(order.note, None);
    assert_eq!(order.shipping.unwrap().city.as_deref(), Some("Tehran"));

    let body = OrderOrder { id: 1, state: OrderState::StatePaid, ..Default::default() };
    block_on(swagger2::apis::put_orders_id(&mock, 1, &body)).unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&mock.last().body).unwrap(), json!({"id": 1, "state": 1}));
}

#[test]
fn openapi31_type_arrays_and_nullable_unions() {
    let mock = Mock::json(json!({
        "id": "a", "kind": "item", "label": null, "price": null,
        "parent": {"id": "b", "kind": "item", "label": "x"}, "tags": ["t", null]
    }));
    let item = block_on(openapi31::apis::get_items_id(&mock, "a")).unwrap();
    assert_eq!(item.label, None);
    assert_eq!(item.parent.unwrap().label.as_deref(), Some("x"));
    assert_eq!(item.tags, Some(vec![Some("t".into()), None]));
}

#[test]
fn object_query_parameters_follow_their_style() {
    use strict::models::{Filter, GetFeedSpringQueryParams, Pageable};
    let mock = Mock::json(json!({"content": [], "last": true}));
    let query = GetFeedSpringQueryParams {
        pageable: Pageable { page: Some(0), size: Some(20), sort: Some(vec!["name".into()]) },
        filter: Some(Filter { color: Some("red".into()) }),
        ..Default::default()
    };
    let page = block_on(strict::apis::get_feed_spring(&mock, &query)).unwrap();
    assert_eq!(page.last, Some(true));
    // `style: form` (the default) sends the object's properties; `deepObject` nests them.
    assert_eq!(
        mock.last().url,
        "https://eu.petstore.example/v1/feed/spring?page=0&size=20&sort=name&filter%5Bcolor%5D=red"
    );
}

#[test]
fn renamed_properties_use_wire_names_and_accept_the_documents() {
    // `title_fa` and `default_variant.seller_id` as the wire spells them,
    // `isInactive` as the document does.
    let body = json!({"id": 1, "title_fa": "x", "isInactive": true, "already_snake": "s", "default_variant": {"seller_id": 5}});

    let product = block_on(naming_lenient::apis::get_products_product_id(&Mock::json(body.clone()), 1)).unwrap();
    assert_eq!(product.title_fa, "x");
    assert_eq!(product.is_inactive, Some(true));
    assert_eq!(product.already_snake.as_deref(), Some("s"));
    assert_eq!(product.default_variant.unwrap().seller_id, Some(5));

    let product = block_on(naming_strict::apis::get_products_product_id(&Mock::json(body), 1)).unwrap();
    assert_eq!(product.title_fa, "x");
    assert_eq!(product.is_inactive, Some(true));
    assert_eq!(product.default_variant.as_ref().unwrap().seller_id, Some(5));

    // Requests use the wire spelling.
    let mock = Mock::json(json!({"title_fa": "y"}));
    block_on(naming_strict::apis::post_products(&mock, &product)).unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&mock.last().body).unwrap();
    assert_eq!(sent["title_fa"], "x");
    assert_eq!(sent["is_inactive"], true);
    assert_eq!(sent["default_variant"]["seller_id"], 5);
    assert!(sent.get("titleFa").is_none());
}
