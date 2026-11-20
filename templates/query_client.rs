//! GPUI integration: GPUI's HTTP client as the [`Transport`], and cached
//! queries, the GPUI counterpart of React Query.
//!
//! * Every `cx.http_client()` is a transport: `apis::get_pet(&cx.http_client(), …)`.
//! * The functions in `queries` return an `Entity<Query<T>>` that fetches on
//!   a background thread, keeps the last data while refetching, retries
//!   network and server errors, and is shared: asking twice for the same
//!   endpoint and parameters gives the same entity and one request.
//! * Data outlives the entities for [`QueryOptions::cache_time`], so a view
//!   that comes back shows it at once while it refreshes.
//! * After a change, [`invalidate_endpoint`] (or [`invalidate`]) refetches
//!   the affected queries that are in use.
//!
//! ```ignore
//! struct PetView {
//!     pet: Entity<Query<models::Pet>>,
//!     _observe: Subscription,
//! }
//!
//! impl PetView {
//!     fn new(id: i64, cx: &mut Context<Self>) -> Self {
//!         let pet = queries::get_pets_pet_id(cx, id);
//!         let _observe = cx.observe(&pet, |_, _, cx| cx.notify());
//!         Self { pet, _observe }
//!     }
//! }
//!
//! impl Render for PetView {
//!     fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
//!         match self.pet.read(cx).state() {
//!             QueryState::Loading => div().child("Loading…"),
//!             QueryState::Error(error) => div().child(error.to_string()),
//!             QueryState::Success(pet) => div().child(pet.name.clone()),
//!         }
//!     }
//! }
//! ```
#![allow(dead_code, clippy::pedantic, clippy::nursery)]

use super::runtime::{ApiError, BoxFuture, HttpRequest, HttpResponse, Transport, TransportError};
use ::__GPUI__::http_client::{self, AsyncBody, HttpClient, HttpRequestExt as _, RedirectPolicy};
use ::__GPUI__::{App, AppContext as _, BackgroundExecutor, Context, Entity, Global, Task, WeakEntity};
use futures::AsyncReadExt as _;
use serde::Serialize;
use serde_json::Value;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ─── Transport ──────────────────────────────────────────────────────────────

/// GPUI's HTTP client (`cx.http_client()`) sends the generated requests.
/// Redirects are followed (up to 10).
impl Transport for dyn HttpClient {
    fn send(&self, request: HttpRequest) -> BoxFuture<'static, Result<HttpResponse, TransportError>> {
        let HttpRequest { method, url, headers, body, timeout } = request;
        let mut builder = http_client::Request::builder()
            .method(method.as_str())
            .uri(url.as_str())
            .follow_redirects(RedirectPolicy::FollowLimit(10));
        __TIMEOUT__
        for (name, value) in &headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let body = if body.is_empty() { AsyncBody::empty() } else { AsyncBody::from(body) };
        let request = match builder.body(body) {
            Ok(request) => request,
            Err(error) => {
                let error = TransportError::new(format!("invalid request: {error}"));
                return Box::pin(async move { Err(error) });
            }
        };
        let response = HttpClient::send(self, request);
        Box::pin(async move {
            let mut response = response.await.map_err(|error| TransportError::new(format!("{error:#}")))?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str().to_owned(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
                .collect();
            let mut body = Vec::new();
            response
                .body_mut()
                .read_to_end(&mut body)
                .await
                .map_err(|error| TransportError::new(format!("reading the body: {error}")))?;
            Ok(HttpResponse { status, headers, body })
        })
    }
}

/// Runs an API call on the background executor, through GPUI's HTTP
/// client; for calls that aren't cached queries (mutations):
///
/// ```ignore
/// let task = query_client::request(cx, move |http| async move { apis::post_pets(&http, &pet).await });
/// cx.spawn(async move |this, cx| {
///     let result = task.await;
///     this.update(cx, |this, cx| {
///         query_client::invalidate_endpoint(cx, queries::GET_PETS);
///         // …
///     })
/// })
/// .detach();
/// ```
pub fn request<T, F, Fut>(cx: &App, call: F) -> Task<Result<T, ApiError>>
where
    T: Send + 'static,
    F: FnOnce(Arc<dyn HttpClient>) -> Fut,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    cx.background_executor().spawn(call(cx.http_client()))
}

// ─── Keys and options ───────────────────────────────────────────────────────

/// Identifies cached data: the endpoint (`"GET /pets/{id}"`) and the
/// parameters it was called with.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct QueryKey {
    endpoint: &'static str,
    params: Arc<str>,
}

impl QueryKey {
    pub fn new(endpoint: &'static str, params: &impl Serialize) -> Self {
        let params = serde_json::to_string(params).unwrap_or_default();
        Self { endpoint, params: params.into() }
    }

    pub fn endpoint(&self) -> &'static str {
        self.endpoint
    }

    /// The parameters, as JSON.
    pub fn params(&self) -> &str {
        &self.params
    }
}

impl fmt::Debug for QueryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.endpoint, self.params)
    }
}

impl fmt::Display for QueryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// How long data stays fresh. A query asked for while its data is
    /// fresh doesn't refetch. Default 0: always refresh, showing cached
    /// data meanwhile.
    pub stale_time: Duration,
    /// How long the data of a query nothing uses is kept. Default 5 min.
    pub cache_time: Duration,
    /// Retries after a retryable error (no response, 408, 429, 5xx).
    /// Default 3.
    pub retry: u32,
    /// The wait before retry `n` (from 0). Default 1 s, doubling, at most
    /// 30 s.
    pub retry_delay: fn(u32) -> Duration,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            stale_time: Duration::ZERO,
            cache_time: Duration::from_secs(5 * 60),
            retry: 3,
            retry_delay: |attempt| Duration::from_secs(1u64 << attempt.min(5)).min(Duration::from_secs(30)),
        }
    }
}

// ─── Cache ──────────────────────────────────────────────────────────────────

/// The cache behind every query of this client.
#[derive(Default)]
pub struct QueryClient {
    entries: HashMap<QueryKey, Entry>,
    options: QueryOptions,
    prefetching: HashSet<QueryKey>,
    swept_at: Option<Instant>,
}

impl Global for QueryClient {}

struct Entry {
    data: Option<Arc<dyn Any + Send + Sync>>,
    updated_at: Option<Instant>,
    invalidated: bool,
    live: Option<Live>,
    used_at: Instant,
}

/// Refetches a live entity; `false` when it is gone.
type Refetch = Rc<dyn Fn(&mut App) -> bool>;

/// The entity currently showing an entry, type-erased.
struct Live {
    /// A `WeakEntity<Query<T>>` or `WeakEntity<InfiniteQuery<T>>`.
    handle: Box<dyn Any>,
    alive: Rc<dyn Fn() -> bool>,
    refetch: Refetch,
}

impl Entry {
    fn new() -> Self {
        Self { data: None, updated_at: None, invalidated: false, live: None, used_at: Instant::now() }
    }
}

/// Cached data, when it was fetched, and whether it was invalidated since.
type Cached<D> = (Arc<D>, Option<Instant>, bool);

impl QueryClient {
    fn cached<D: Send + Sync + 'static>(&mut self, key: &QueryKey) -> Option<Cached<D>> {
        let entry = self.entries.get_mut(key)?;
        entry.used_at = Instant::now();
        let data = entry.data.clone()?.downcast::<D>().ok()?;
        Some((data, entry.updated_at, entry.invalidated))
    }

    fn store<D: Send + Sync + 'static>(&mut self, key: &QueryKey, data: Arc<D>) {
        let entry = self.entries.entry(key.clone()).or_insert_with(Entry::new);
        entry.data = Some(data);
        entry.updated_at = Some(Instant::now());
        entry.invalidated = false;
        entry.used_at = Instant::now();
    }

    /// Drops data nothing has used for `cache_time`, at most every 30 s.
    fn sweep(&mut self) {
        let now = Instant::now();
        if self.swept_at.is_some_and(|at| now.duration_since(at) < Duration::from_secs(30)) {
            return;
        }
        self.swept_at = Some(now);
        let cache_time = self.options.cache_time;
        self.entries.retain(|_, entry| {
            let alive = entry.live.as_ref().is_some_and(|live| (live.alive)());
            if !alive {
                entry.live = None;
            }
            alive || now.duration_since(entry.used_at) < cache_time
        });
    }
}

fn client(cx: &mut App) -> &mut QueryClient {
    cx.default_global::<QueryClient>()
}

fn live_entity<E: 'static>(cx: &mut App, key: &QueryKey) -> Option<Entity<E>> {
    let entry = client(cx).entries.get_mut(key)?;
    entry.used_at = Instant::now();
    entry.live.as_ref()?.handle.downcast_ref::<WeakEntity<E>>()?.upgrade()
}

fn register<E: 'static>(
    cx: &mut App,
    key: QueryKey,
    entity: &Entity<E>,
    refetch: impl Fn(&mut E, &mut Context<E>) + 'static,
) {
    let weak = entity.downgrade();
    let alive = weak.clone();
    let refetch_target = weak.clone();
    let live = Live {
        handle: Box::new(weak),
        alive: Rc::new(move || alive.upgrade().is_some()),
        refetch: Rc::new(move |cx: &mut App| refetch_target.update(cx, |entity, cx| refetch(entity, cx)).is_ok()),
    };
    let entry = client(cx).entries.entry(key).or_insert_with(Entry::new);
    entry.live = Some(live);
    entry.used_at = Instant::now();
}

/// Sets the options new queries start with.
pub fn set_default_options(cx: &mut App, options: QueryOptions) {
    client(cx).options = options;
}

/// Marks the matching data stale and refetches the queries in use.
pub fn invalidate(cx: &mut App, filter: impl Fn(&QueryKey) -> bool) {
    let refetches: Vec<Refetch> = client(cx)
        .entries
        .iter_mut()
        .filter(|(key, _)| filter(key))
        .filter_map(|(_, entry)| {
            entry.invalidated = true;
            entry.live.as_ref().map(|live| live.refetch.clone())
        })
        .collect();
    for refetch in refetches {
        refetch(cx);
    }
}

/// [`invalidate`]s every query of `endpoint` (the `queries::GET_…`
/// constants), whatever its parameters.
pub fn invalidate_endpoint(cx: &mut App, endpoint: &str) {
    invalidate(cx, |key| key.endpoint() == endpoint);
}

pub fn invalidate_all(cx: &mut App) {
    invalidate(cx, |_| true);
}

/// Forgets all cached data (on logout, say). Queries in use keep theirs
/// until they refetch.
pub fn clear(cx: &mut App) {
    client(cx).entries.retain(|_, entry| {
        entry.data = None;
        entry.live.is_some()
    });
}

/// The cached data of a [`Query`].
pub fn query_data<T: Send + Sync + 'static>(cx: &mut App, key: &QueryKey) -> Option<Arc<T>> {
    client(cx).cached::<T>(key).map(|(data, _, _)| data)
}

/// Replaces the data of a [`Query`] (after a mutation returned the new
/// value, say), in the cache and in the entity showing it.
pub fn set_query_data<T: Send + Sync + 'static>(cx: &mut App, key: &QueryKey, data: T) {
    let data = Arc::new(data);
    client(cx).store(key, data.clone());
    if let Some(entity) = live_entity::<Query<T>>(cx, key) {
        entity.update(cx, |query, cx| {
            query.data = Some(data);
            query.error = None;
            query.updated_at = Some(Instant::now());
            query.invalidated = false;
            cx.notify();
        });
    }
}

async fn fetch_with_retry<T: Send + 'static>(
    executor: &BackgroundExecutor,
    options: &QueryOptions,
    fetch: impl Fn() -> BoxFuture<'static, Result<T, ApiError>>,
) -> Result<T, ApiError> {
    let mut attempt = 0;
    loop {
        match executor.spawn(fetch()).await {
            Err(error) if attempt < options.retry && error.is_retryable() => {
                executor.timer((options.retry_delay)(attempt)).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}

// ─── Query ──────────────────────────────────────────────────────────────────

type Fetcher<T> = Rc<dyn Fn(Arc<dyn HttpClient>) -> BoxFuture<'static, Result<T, ApiError>>>;

/// What a [`Query`] has to show.
pub enum QueryState<'a, T> {
    /// Nothing yet: the first fetch is running (or about to).
    Loading,
    /// The fetch failed and there is no earlier data.
    Error(&'a ApiError),
    /// Data, possibly being refreshed ([`Query::is_fetching`]) or followed
    /// by a failed refresh ([`Query::error`]).
    Success(&'a Arc<T>),
}

/// The data of one endpoint call, kept up to date. Create it with the
/// functions in `queries`, observe it, and read [`Query::state`].
pub struct Query<T> {
    key: QueryKey,
    options: QueryOptions,
    data: Option<Arc<T>>,
    error: Option<ApiError>,
    updated_at: Option<Instant>,
    invalidated: bool,
    fetcher: Fetcher<T>,
    task: Option<Task<()>>,
}

/// The cached [`Query`] for `key` (created and fetched if needed). The
/// generated `queries` functions call this.
pub fn query<T, F, Fut>(cx: &mut App, key: QueryKey, fetch: F) -> Entity<Query<T>>
where
    T: Send + Sync + 'static,
    F: Fn(Arc<dyn HttpClient>) -> Fut + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    client(cx).sweep();
    if let Some(entity) = live_entity::<Query<T>>(cx, &key) {
        entity.update(cx, |query, cx| query.ensure_fresh(cx));
        return entity;
    }
    let options = client(cx).options.clone();
    let cached = client(cx).cached::<T>(&key);
    let fetcher: Fetcher<T> = Rc::new(move |http: Arc<dyn HttpClient>| -> BoxFuture<'static, Result<T, ApiError>> {
        Box::pin(fetch(http))
    });
    let entity = cx.new(|cx| {
        let (data, updated_at, invalidated) = match cached {
            Some((data, updated_at, invalidated)) => (Some(data), updated_at, invalidated),
            None => (None, None, false),
        };
        let mut query =
            Query { key: key.clone(), options, data, error: None, updated_at, invalidated, fetcher, task: None };
        query.ensure_fresh(cx);
        query
    });
    register(cx, key, &entity, |query: &mut Query<T>, cx| query.refetch(cx));
    entity
}

/// Fetches `key` into the cache unless it is fresh or already loading, so a
/// [`Query`] created later shows it at once.
pub fn prefetch<T, F, Fut>(cx: &mut App, key: QueryKey, fetch: F)
where
    T: Send + Sync + 'static,
    F: Fn(Arc<dyn HttpClient>) -> Fut + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    if let Some(entity) = live_entity::<Query<T>>(cx, &key) {
        entity.update(cx, |query, cx| query.ensure_fresh(cx));
        return;
    }
    let options = client(cx).options.clone();
    if let Some((_, updated_at, invalidated)) = client(cx).cached::<T>(&key) {
        let fresh = !invalidated && updated_at.is_some_and(|at| at.elapsed() < options.stale_time);
        if fresh {
            return;
        }
    }
    if !client(cx).prefetching.insert(key.clone()) {
        return;
    }
    let http = cx.http_client();
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let result = fetch_with_retry(&executor, &options, || -> BoxFuture<'static, Result<T, ApiError>> {
            Box::pin(fetch(http.clone()))
        })
        .await;
        // `AsyncApp::update` returns `()` in newer GPUI and a `Result` in
        // older versions; `let _` compiles with both.
        #[allow(clippy::let_unit_value)]
        let _ = cx.update(|cx| {
            client(cx).prefetching.remove(&key);
            if let Ok(data) = result {
                set_query_data(cx, &key, data);
            }
        });
    })
    .detach();
}

impl<T: Send + Sync + 'static> Query<T> {
    pub fn key(&self) -> &QueryKey {
        &self.key
    }

    pub fn state(&self) -> QueryState<'_, T> {
        match (&self.data, &self.error) {
            (Some(data), _) => QueryState::Success(data),
            (None, Some(error)) if !self.is_fetching() => QueryState::Error(error),
            _ => QueryState::Loading,
        }
    }

    pub fn data(&self) -> Option<&Arc<T>> {
        self.data.as_ref()
    }

    /// The error of the last fetch, if it failed.
    pub fn error(&self) -> Option<&ApiError> {
        self.error.as_ref()
    }

    pub fn is_fetching(&self) -> bool {
        self.task.is_some()
    }

    /// No data yet and a fetch running.
    pub fn is_loading(&self) -> bool {
        self.data.is_none() && self.is_fetching()
    }

    pub fn is_stale(&self) -> bool {
        self.invalidated || self.updated_at.is_none_or(|at| at.elapsed() >= self.options.stale_time)
    }

    /// When the data was fetched.
    pub fn updated_at(&self) -> Option<Instant> {
        self.updated_at
    }

    pub fn options(&self) -> &QueryOptions {
        &self.options
    }

    pub fn set_options(&mut self, options: QueryOptions) {
        self.options = options;
    }

    /// Fetches again (cancelling a fetch in progress).
    pub fn refetch(&mut self, cx: &mut Context<Self>) {
        let fetch = self.fetcher.clone();
        let http = cx.http_client();
        let executor = cx.background_executor().clone();
        let options = self.options.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = fetch_with_retry(&executor, &options, || fetch(http.clone())).await;
            this.update(cx, |query, cx| query.finish(result, cx)).ok();
        }));
        cx.notify();
    }

    /// Fetches when the data is stale and no fetch is running.
    pub fn ensure_fresh(&mut self, cx: &mut Context<Self>) {
        if self.is_stale() && !self.is_fetching() {
            self.refetch(cx);
        }
    }

    /// Replaces the data (cache included).
    pub fn set_data(&mut self, data: T, cx: &mut Context<Self>) {
        let data = Arc::new(data);
        client(cx).store(&self.key, data.clone());
        self.data = Some(data);
        self.error = None;
        self.updated_at = Some(Instant::now());
        self.invalidated = false;
        cx.notify();
    }

    fn finish(&mut self, result: Result<T, ApiError>, cx: &mut Context<Self>) {
        self.task = None;
        match result {
            Ok(data) => {
                let data = Arc::new(data);
                client(cx).store(&self.key, data.clone());
                self.data = Some(data);
                self.error = None;
                self.updated_at = Some(Instant::now());
                self.invalidated = false;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}

// ─── Infinite query ─────────────────────────────────────────────────────────

type PageFetcher<T> = Rc<dyn Fn(Arc<dyn HttpClient>, i64) -> BoxFuture<'static, Result<T, ApiError>>>;
type HasNextPage<T> = Rc<dyn Fn(&T, &[Arc<T>]) -> bool>;

/// What an [`InfiniteQuery`] keeps in the cache.
struct Pages<T> {
    pages: Vec<Arc<T>>,
    has_next: bool,
    items: usize,
}

/// A paged endpoint (React Query's `useInfiniteQuery`): pages 1, 2, 3…
/// loaded one after the other with [`InfiniteQuery::fetch_next_page`].
pub struct InfiniteQuery<T> {
    key: QueryKey,
    options: QueryOptions,
    pages: Vec<Arc<T>>,
    has_next: bool,
    /// Items in the loaded pages' lists, for `total` based paging.
    items: usize,
    error: Option<ApiError>,
    updated_at: Option<Instant>,
    invalidated: bool,
    fetcher: PageFetcher<T>,
    has_next_page: Option<HasNextPage<T>>,
    task: Option<Task<()>>,
    /// The page being fetched.
    fetching_page: Option<i64>,
}

/// The cached [`InfiniteQuery`] for `key`; `fetch` gets the page number
/// (from 1).
pub fn infinite_query<T, F, Fut>(cx: &mut App, key: QueryKey, fetch: F) -> Entity<InfiniteQuery<T>>
where
    T: Serialize + Send + Sync + 'static,
    F: Fn(Arc<dyn HttpClient>, i64) -> Fut + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    client(cx).sweep();
    if let Some(entity) = live_entity::<InfiniteQuery<T>>(cx, &key) {
        entity.update(cx, |query, cx| query.ensure_fresh(cx));
        return entity;
    }
    let options = client(cx).options.clone();
    let cached = client(cx).cached::<Pages<T>>(&key);
    let fetcher: PageFetcher<T> =
        Rc::new(move |http: Arc<dyn HttpClient>, page: i64| -> BoxFuture<'static, Result<T, ApiError>> {
            Box::pin(fetch(http, page))
        });
    let entity = cx.new(|cx| {
        let mut query = InfiniteQuery {
            key: key.clone(),
            options,
            pages: Vec::new(),
            has_next: true,
            items: 0,
            error: None,
            updated_at: None,
            invalidated: false,
            fetcher,
            has_next_page: None,
            task: None,
            fetching_page: None,
        };
        if let Some((cached, updated_at, invalidated)) = cached {
            query.pages = cached.pages.clone();
            query.has_next = cached.has_next;
            query.items = cached.items;
            query.updated_at = updated_at;
            query.invalidated = invalidated;
        }
        query.ensure_fresh(cx);
        query
    });
    register(cx, key, &entity, |query: &mut InfiniteQuery<T>, cx| query.refetch(cx));
    entity
}

impl<T: Serialize + Send + Sync + 'static> InfiniteQuery<T> {
    pub fn key(&self) -> &QueryKey {
        &self.key
    }

    /// The loaded pages, in order.
    pub fn pages(&self) -> &[Arc<T>] {
        &self.pages
    }

    pub fn has_next_page(&self) -> bool {
        self.has_next
    }

    pub fn error(&self) -> Option<&ApiError> {
        self.error.as_ref()
    }

    pub fn is_fetching(&self) -> bool {
        self.task.is_some()
    }

    pub fn is_fetching_next_page(&self) -> bool {
        self.fetching_page.is_some_and(|page| page > 1)
    }

    /// No page yet and a fetch running.
    pub fn is_loading(&self) -> bool {
        self.pages.is_empty() && self.is_fetching()
    }

    pub fn is_stale(&self) -> bool {
        self.invalidated || self.updated_at.is_none_or(|at| at.elapsed() >= self.options.stale_time)
    }

    pub fn set_options(&mut self, options: QueryOptions) {
        self.options = options;
    }

    /// Decides whether there is a page after the last one. The default
    /// reads the page's JSON: a `has_next`/`has_more` flag, `last` (Spring),
    /// current vs total pages, loaded items vs `total`, else "the last page
    /// wasn't empty" — look for these under the page itself or its `pager`,
    /// `pagination` or `meta`.
    pub fn set_has_next_page(&mut self, has_next_page: impl Fn(&T, &[Arc<T>]) -> bool + 'static) {
        self.has_next_page = Some(Rc::new(has_next_page));
        if let Some(last) = self.pages.last() {
            self.has_next = (self.has_next_page.as_ref().expect("just set"))(last, &self.pages);
        }
    }

    /// Loads the next page, unless there is none or a fetch is running.
    pub fn fetch_next_page(&mut self, cx: &mut Context<Self>) {
        if self.has_next && !self.is_fetching() && !self.pages.is_empty() {
            self.fetch(self.pages.len() as i64 + 1, cx);
        }
    }

    /// Starts over from page 1; the loaded pages stay until it arrives.
    pub fn refetch(&mut self, cx: &mut Context<Self>) {
        self.fetch(1, cx);
    }

    pub fn ensure_fresh(&mut self, cx: &mut Context<Self>) {
        if (self.pages.is_empty() || self.is_stale()) && !self.is_fetching() {
            self.refetch(cx);
        }
    }

    fn fetch(&mut self, page: i64, cx: &mut Context<Self>) {
        let fetch = self.fetcher.clone();
        let http = cx.http_client();
        let executor = cx.background_executor().clone();
        let options = self.options.clone();
        self.fetching_page = Some(page);
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = fetch_with_retry(&executor, &options, || fetch(http.clone(), page)).await;
            this.update(cx, |query, cx| query.finish(page, result, cx)).ok();
        }));
        cx.notify();
    }

    fn finish(&mut self, page: i64, result: Result<T, ApiError>, cx: &mut Context<Self>) {
        self.task = None;
        self.fetching_page = None;
        match result {
            Ok(data) => {
                let data = Arc::new(data);
                let json = serde_json::to_value(&*data).unwrap_or(Value::Null);
                if page == 1 {
                    self.pages.clear();
                    self.items = 0;
                    self.updated_at = Some(Instant::now());
                    self.invalidated = false;
                }
                self.items += list_len(&json);
                self.pages.push(data.clone());
                self.has_next = match &self.has_next_page {
                    Some(has_next_page) => has_next_page(&data, &self.pages),
                    None => default_has_next_page(&json, self.items),
                };
                self.error = None;
                let cached = Pages { pages: self.pages.clone(), has_next: self.has_next, items: self.items };
                client(cx).store(&self.key, Arc::new(cached));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}

const PAGE_CONTAINERS: &[&str] = &["pager", "pagination", "meta", "page_info", "pageInfo", "paging"];
const LIST_KEYS: &[&str] = &["data", "list", "items", "results", "content", "products", "records", "rows"];

fn page_objects(page: &Value) -> Vec<&Value> {
    let mut objects = vec![page];
    objects.extend(PAGE_CONTAINERS.iter().filter_map(|key| page.get(*key)).filter(|v| v.is_object()));
    if let Some(data) = page.get("data").filter(|v| v.is_object()) {
        objects.push(data);
        objects.extend(PAGE_CONTAINERS.iter().filter_map(|key| data.get(*key)).filter(|v| v.is_object()));
    }
    objects
}

fn first_number(objects: &[&Value], keys: &[&str]) -> Option<(f64, &'static str)> {
    for object in objects {
        for key in keys {
            let number = match object.get(*key) {
                Some(Value::Number(n)) => n.as_f64(),
                Some(Value::String(s)) => s.trim().parse().ok(),
                _ => None,
            };
            if let Some(number) = number {
                // `number` (Spring) counts from 0; the others from 1.
                let base = if *key == "number" { "zero" } else { "one" };
                return Some((number, base));
            }
        }
    }
    None
}

/// The length of the page's list: the page itself, or its first array under
/// `data`, `list`, `items`…, or its first array at all.
fn list_len(page: &Value) -> usize {
    match page {
        Value::Array(items) => items.len(),
        Value::Object(map) => LIST_KEYS
            .iter()
            .filter_map(|key| map.get(*key))
            .find_map(|value| match value {
                Value::Array(items) => Some(items.len()),
                Value::Object(_) => Some(list_len(value)),
                _ => None,
            })
            .or_else(|| map.values().find_map(|value| value.as_array().map(Vec::len)))
            .unwrap_or(0),
        _ => 0,
    }
}

fn default_has_next_page(page: &Value, items: usize) -> bool {
    let objects = page_objects(page);
    for object in &objects {
        for key in ["has_next", "hasNext", "has_more", "hasMore", "has_next_page", "hasNextPage"] {
            if let Some(Value::Bool(flag)) = object.get(key) {
                return *flag;
            }
        }
    }
    // Spring's `Page`, possibly inside an envelope's `data`.
    for object in &objects {
        if let Some(Value::Bool(last)) = object.get("last") {
            return !last;
        }
    }
    let current = first_number(&objects, &["current_page", "currentPage", "page_number", "pageNumber", "page", "number"]);
    let total_pages =
        first_number(&objects, &["total_pages", "totalPages", "last_page", "lastPage", "page_count", "pageCount"]);
    if let (Some((current, base)), Some((total, _))) = (current, total_pages) {
        let current = if base == "zero" { current + 1.0 } else { current };
        return current < total;
    }
    if let Some((total, _)) =
        first_number(&objects, &["total", "total_count", "totalCount", "total_items", "totalItems", "totalElements"])
    {
        return (items as f64) < total;
    }
    list_len(page) > 0
}
