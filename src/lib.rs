//! Modio provides a set of building blocks for interacting with the [mod.io](https://mod.io) API.
//!
//! The client uses asynchronous I/O, backed by the `futures` and `tokio` crates, and requires both
//! to be used alongside.
//!
//! # Authentication
//!
//! To access the API authentication is required and can be done via 4 ways:
//!
//! - Request an [API key (Read-only)](https://mod.io/apikey)
//! - Manually create an [OAuth 2 Access Token (Read + Write)](https://mod.io/oauth)
//! - [Email Authentication Flow](auth/struct.Auth.html#example) to create an OAuth 2 Access Token
//! (Read + Write)
//! - [Encrypted steam user auth ticket](auth/struct.Auth.html#method.steam_auth) to create an
//! OAuth 2 Access Token (Read + Write)
//!
//! # Rate Limiting
//!
//! For API requests using API key authentication are **unlimited** and for OAuth 2 authentication
//! requests are limited to **120 requests per hour**.
//!
//! A special error [ErrorKind::RateLimit](error/enum.ErrorKind.html#variant.RateLimit) will
//! be return from api operations when the rate limit associated with credentials has been
//! exhausted.
//!
//! # Example: Basic setup
//!
//! ```no_run
//! use modio::{Credentials, Error, Modio};
//! use tokio::runtime::Runtime;
//!
//! fn main() -> Result<(), Error> {
//!     let mut rt = Runtime::new()?;
//!     let modio = Modio::new(
//!         "user-agent-name/1.0",
//!         Credentials::ApiKey(String::from("user-or-game-api-key")),
//!     );
//!
//!     // create some tasks and execute them
//!     // let result = rt.block_on(task)?;
//!     Ok(())
//! }
//! ```
//!
//! For testing purposes use [`Modio::host`](struct.Modio.html#method.host) to create a client for the
//! mod.io [test environment](https://docs.mod.io/#testing).
//!
//! # Example: Chaining api requests
//!
//! ```no_run
//! use modio::{Credentials, Error, Modio};
//! use tokio::prelude::*;
//! use tokio::runtime::Runtime;
//!
//! fn main() -> Result<(), Error> {
//!     let mut rt = Runtime::new()?;
//!     let modio = Modio::new(
//!         "user-agent-name/1.0",
//!         Credentials::ApiKey(String::from("user-or-game-api-key")),
//!     )?;
//!
//!     // OpenXcom: The X-Com Files
//!     let modref = modio.mod_(51, 158);
//!
//!     // Get mod with its dependencies and all files
//!     let mod_ = modref.get();
//!     let deps = modref.dependencies().list();
//!     let files = modref.files().list(&Default::default());
//!
//!     let task = mod_.join(deps).join(files);
//!
//!     match rt.block_on(task) {
//!         Ok(((m, deps), files)) => {
//!             println!("{}", m.name);
//!             println!(
//!                 "deps: {:?}",
//!                 deps.into_iter().map(|d| d.mod_id).collect::<Vec<_>>()
//!             );
//!             for file in files {
//!                 println!("file id: {} version: {:?}", file.id, file.version);
//!             }
//!         }
//!         Err(e) => println!("{}", e),
//!     };
//!     Ok(())
//! }
//! ```
//!
//! # Example: Downloading mods
//!
//! ```no_run
//! use std::fs::File;
//!
//! use modio::download::ResolvePolicy;
//! use modio::{Credentials, DownloadAction, Error, Modio};
//! use tokio::runtime::Runtime;
//!
//! fn main() -> Result<(), Error> {
//!     let mut rt = Runtime::new()?;
//!     let modio = Modio::new(
//!         "user-agent-name/1.0",
//!         Credentials::ApiKey(String::from("user-or-game-api-key")),
//!     )?;
//!     let out = File::open("mod.zip")?;
//!
//!     // Download the primary file of a mod.
//!     let action = DownloadAction::Primary {
//!         game_id: 5,
//!         mod_id: 19,
//!     };
//!     let (len, out) = rt.block_on(modio.download(action, out))?;
//!
//!     // Download the specific file of a mod.
//!     let action = DownloadAction::File {
//!         game_id: 5,
//!         mod_id: 19,
//!         file_id: 101,
//!     };
//!     let (len, out) = rt.block_on(modio.download(action, out))?;
//!
//!     // Download the specific version of a mod.
//!     // if multiple files are found then the latest file is downloaded.
//!     // Set policy to `ResolvePolicy::Fail` to return with
//!     // `ErrorKind::Download(DownloadError::MultipleFilesFound)`.
//!     let action = DownloadAction::Version {
//!         game_id: 5,
//!         mod_id: 19,
//!         version: "0.1".to_string(),
//!         policy: ResolvePolicy::Latest,
//!     };
//!     let (len, out) = rt.block_on(modio.download(action, out))?;
//!     Ok(())
//! }
//! ```

#![doc(html_root_url = "https://docs.rs/modio/0.3.0")]

#[macro_use]
extern crate serde_derive;

use std::collections::BTreeMap;
use std::io;
use std::io::prelude::*;
use std::marker::PhantomData;
use std::time::Duration;

use futures::{future, stream, Future as StdFuture, IntoFuture, Stream as StdStream};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, LOCATION, USER_AGENT};
use hyper::{Method, StatusCode};
use mime::Mime;
use reqwest::r#async::multipart::Form;
use reqwest::r#async::{Body, Client};
use serde::de::DeserializeOwned;
use url::Url;

pub mod auth;
#[macro_use]
pub mod filter;
pub mod comments;
pub mod download;
#[macro_use]
pub mod error;
pub mod files;
pub mod games;
pub mod me;
pub mod metadata;
pub mod mods;
mod multipart;
pub mod reports;
pub mod teams;
mod types;
pub mod users;

use crate::auth::Auth;
use crate::comments::Comments;
use crate::games::{GameRef, Games};
use crate::me::Me;
use crate::mods::{ModRef, Mods};
use crate::reports::Reports;
use crate::users::Users;

pub use crate::auth::Credentials;
pub use crate::download::DownloadAction;
pub use crate::error::{Error, Result};
pub use crate::types::{ModioErrorResponse, ModioListResponse, ModioMessage};

const DEFAULT_HOST: &str = "https://api.mod.io/v1";

pub type Future<T> = Box<dyn StdFuture<Item = T, Error = Error> + Send>;
pub type Stream<T> = Box<dyn StdStream<Item = T, Error = Error> + Send>;
pub type List<T> = ModioListResponse<T>;

mod prelude {
    pub use futures::{Future as StdFuture, Stream as StdStream};
    pub use reqwest::r#async::multipart::{Form, Part};
    pub use reqwest::r#async::Body;

    pub use crate::List;
    pub use crate::Modio;
    pub use crate::ModioMessage;
    pub use crate::QueryParams;
    pub(crate) use crate::RequestBody;
    pub use crate::{AddOptions, DeleteOptions, Endpoint};
    pub use crate::{Future, Stream};
}

#[allow(dead_code)]
const X_RATELIMIT_LIMIT: &str = "x-ratelimit-limit";
const X_RATELIMIT_REMAINING: &str = "x-ratelimit-remaining";
const X_RATELIMIT_RETRY_AFTER: &str = "x-ratelimit-retryafter";

/// Endpoint interface to interacting with the [mod.io](https://mod.io) API.
#[derive(Clone, Debug)]
pub struct Modio {
    host: String,
    agent: String,
    client: Client,
    credentials: Credentials,
}

impl Modio {
    /// Create an endpoint to [https://api.mod.io/v1](https://docs.mod.io/#mod-io-api-v1).
    pub fn new<A, C>(agent: A, credentials: C) -> Result<Self>
    where
        A: Into<String>,
        C: Into<Credentials>,
    {
        Self::host(DEFAULT_HOST, agent, credentials)
    }

    /// Create an endpoint to a different host.
    pub fn host<H, A, C>(host: H, agent: A, credentials: C) -> Result<Self>
    where
        H: Into<String>,
        A: Into<String>,
        C: Into<Credentials>,
    {
        let client = Client::builder().build()?;

        Ok(Self::custom(host, agent, credentials, client))
    }

    /// Create an endpoint with a custom hyper client.
    pub fn custom<H, A, CR>(host: H, agent: A, credentials: CR, client: Client) -> Self
    where
        H: Into<String>,
        A: Into<String>,
        CR: Into<Credentials>,
    {
        Self {
            host: host.into(),
            agent: agent.into(),
            client,
            credentials: credentials.into(),
        }
    }

    /// Consume the endpoint and create an endpoint with new credentials.
    pub fn with_credentials<CR>(self, credentials: CR) -> Self
    where
        CR: Into<Credentials>,
    {
        Self {
            host: self.host,
            agent: self.agent,
            client: self.client,
            credentials: credentials.into(),
        }
    }

    /// Return a reference to an interface for requesting access tokens.
    pub fn auth(&self) -> Auth {
        Auth::new(self.clone())
    }

    /// Return a reference to an interface that provides access to game information.
    pub fn games(&self) -> Games {
        Games::new(self.clone())
    }

    /// Return a reference to a game.
    pub fn game(&self, game_id: u32) -> GameRef {
        GameRef::new(self.clone(), game_id)
    }

    /// Return a reference to a mod.
    pub fn mod_(&self, game_id: u32, mod_id: u32) -> ModRef {
        ModRef::new(self.clone(), game_id, mod_id)
    }

    /// Performs a download into a writer.
    ///
    /// Fails with [`ErrorKind::Download`](error/enum.ErrorKind.html#variant.Download) if a primary file,
    /// a specific file or a specific version is not found.
    /// # Example
    /// ```no_run
    /// use std::fs::File;
    ///
    /// use modio::download::ResolvePolicy;
    /// use modio::{Credentials, DownloadAction, Error, Modio};
    /// use tokio::runtime::Runtime;
    ///
    /// fn main() -> Result<(), Error> {
    ///     let mut rt = Runtime::new()?;
    ///     let modio = Modio::new(
    ///         "user-agent-name/1.0",
    ///         Credentials::ApiKey(String::from("user-or-game-api-key")),
    ///     )?;
    ///     let out = File::open("mod.zip")?;
    ///
    ///     // Download the primary file of a mod.
    ///     let action = DownloadAction::Primary {
    ///         game_id: 5,
    ///         mod_id: 19,
    ///     };
    ///     let (len, out) = rt.block_on(modio.download(action, out))?;
    ///
    ///     // Download the specific file of a mod.
    ///     let action = DownloadAction::File {
    ///         game_id: 5,
    ///         mod_id: 19,
    ///         file_id: 101,
    ///     };
    ///     let (len, out) = rt.block_on(modio.download(action, out))?;
    ///
    ///     // Download the specific version of a mod.
    ///     // if multiple files are found then the latest file is downloaded.
    ///     // Set policy to `ResolvePolicy::Fail` to return with
    ///     // `ErrorKind::Download(DownloadError::MultipleFilesFound)`.
    ///     let action = DownloadAction::Version {
    ///         game_id: 5,
    ///         mod_id: 19,
    ///         version: "0.1".to_string(),
    ///         policy: ResolvePolicy::Latest,
    ///     };
    ///     let (len, out) = rt.block_on(modio.download(action, out))?;
    ///     Ok(())
    /// }
    /// ```
    pub fn download<A, W>(&self, action: A, w: W) -> Future<(u64, W)>
    where
        A: Into<DownloadAction>,
        W: Write + 'static + Send,
    {
        let instance = self.clone();
        match action.into() {
            DownloadAction::Primary { game_id, mod_id } => {
                Box::new(self.mod_(game_id, mod_id).get().and_then(move |m| {
                    if let Some(file) = m.modfile {
                        instance.request_file(&file.download.binary_url.to_string(), w)
                    } else {
                        future_err!(error::download_no_primary(game_id, mod_id))
                    }
                }))
            }
            DownloadAction::File {
                game_id,
                mod_id,
                file_id,
            } => Box::new(
                self.mod_(game_id, mod_id)
                    .file(file_id)
                    .get()
                    .and_then(move |file| {
                        instance.request_file(&file.download.binary_url.to_string(), w)
                    })
                    .map_err(move |e| match e.kind() {
                        error::ErrorKind::Fault {
                            code: StatusCode::NOT_FOUND,
                            ..
                        } => error::download_file_not_found(game_id, mod_id, file_id),
                        _ => e,
                    }),
            ),
            DownloadAction::Version {
                game_id,
                mod_id,
                version,
                policy,
            } => {
                let mut opts = files::FileListOptions::new();
                opts.version(filter::Operator::Equals, version.clone());
                opts.sort_by(files::FileListOptions::DATE_ADDED, filter::Order::Desc);
                opts.limit(2);

                Box::new(
                    self.mod_(game_id, mod_id)
                        .files()
                        .list(&opts)
                        .and_then(move |list| {
                            use crate::download::ResolvePolicy::*;

                            let (file, error) = match (list.count, policy) {
                                (0, _) => (
                                    None,
                                    Some(error::download_version_not_found(
                                        game_id, mod_id, version,
                                    )),
                                ),
                                (1, _) => (Some(&list[0]), None),
                                (_, Latest) => (Some(&list[0]), None),
                                (_, Fail) => (
                                    None,
                                    Some(error::download_multiple_files(game_id, mod_id, version)),
                                ),
                            };

                            if let Some(file) = file {
                                instance.request_file(&file.download.binary_url.to_string(), w)
                            } else {
                                future_err!(error.expect("bug in previous match!"))
                            }
                        }),
                )
            }
            DownloadAction::Url(url) => self.request_file(&url.to_string(), w),
        }
    }

    /// Return a reference to an interface that provides access to resources owned by the user
    /// associated with the current authentication credentials.
    pub fn me(&self) -> Me {
        Me::new(self.clone())
    }

    /// Return a reference to an interface that provides access to user information.
    pub fn users(&self) -> Users {
        Users::new(self.clone())
    }

    /// Return a reference to an interface to report games, mods and users.
    pub fn reports(&self) -> Reports {
        Reports::new(self.clone())
    }

    fn request<B, Out>(&self, method: Method, uri: &str, body: B) -> Future<(Url, Out)>
    where
        B: Into<RequestBody> + 'static + Send,
        Out: DeserializeOwned + 'static + Send,
    {
        let url = if let Credentials::ApiKey(ref api_key) = self.credentials {
            Url::parse(&uri)
                .map(|mut url| {
                    url.query_pairs_mut().append_pair("api_key", api_key);
                    url
                })
                .map_err(Error::from)
                .into_future()
        } else {
            uri.parse().map_err(Error::from).into_future()
        };

        let instance = self.clone();

        let response = url.map_err(Error::from).and_then(move |url| {
            let mut req = instance
                .client
                .request(method, url.clone())
                .header(USER_AGENT, &*instance.agent);

            if let Credentials::Token(ref token) = instance.credentials {
                req = req.header(AUTHORIZATION, &*format!("Bearer {}", token));
            }

            match body.into() {
                RequestBody::Body(body, mime) => {
                    if let Some(mime) = mime {
                        req = req.header(CONTENT_TYPE, &*mime.to_string());
                    }
                    req = req.body(body);
                }
                RequestBody::Form(form) => {
                    req = req.multipart(form);
                }
                _ => {}
            }
            req.send()
                .map_err(Error::from)
                .and_then(|res| Ok((url, res)))
        });

        Box::new(response.and_then(move |(url, response)| {
            let remaining = response
                .headers()
                .get(X_RATELIMIT_REMAINING)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let reset = response
                .headers()
                .get(X_RATELIMIT_RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());

            let status = response.status();
            Box::new(
                response
                    .into_body()
                    .concat2()
                    .map_err(Error::from)
                    .and_then(move |response_body| {
                        if status.is_success() {
                            serde_json::from_slice::<Out>(&response_body)
                                .map(|out| (url, out))
                                .map_err(Error::from)
                        } else {
                            let error = match (remaining, reset) {
                                (Some(remaining), Some(reset)) if remaining == 0 => {
                                    error::ErrorKind::RateLimit {
                                        reset: Duration::from_secs(reset as u64 * 60),
                                    }
                                }
                                _ => {
                                    let mer: ModioErrorResponse =
                                        serde_json::from_slice(&response_body)?;
                                    error::ErrorKind::Fault {
                                        code: status,
                                        error: mer.error,
                                    }
                                }
                            };
                            Err(error.into())
                        }
                    }),
            )
        }))
    }

    fn request_entity<B, D>(&self, method: Method, uri: &str, body: B) -> Future<D>
    where
        B: Into<RequestBody> + 'static + Send,
        D: DeserializeOwned + 'static + Send,
    {
        Box::new(self.request(method, uri, body).map(|(_, entity)| entity))
    }

    fn request_file<W>(&self, uri: &str, out: W) -> Future<(u64, W)>
    where
        W: Write + 'static + Send,
    {
        let url = Url::parse(uri).map_err(Error::from).into_future();

        let instance = self.clone();
        let response = url.and_then(move |url| {
            let mut req = instance.client.request(Method::GET, url);
            req = req.header(USER_AGENT, &*instance.agent);
            req.send().map_err(Error::from)
        });

        let instance2 = self.clone();
        Box::new(response.and_then(move |response| {
            let status = response.status();
            if StatusCode::MOVED_PERMANENTLY == status
                || StatusCode::TEMPORARY_REDIRECT == status
                || StatusCode::FOUND == status
            {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok());
                if let Some(location) = location {
                    return instance2.request_file(&location.to_string(), out);
                }
            }
            Box::new(response.into_body().map_err(Error::from).fold(
                (0, out),
                |(len, mut out), chunk| {
                    io::copy(&mut io::Cursor::new(&chunk), &mut out)
                        .map(|n| (n + len, out))
                        .map_err(Error::from)
                        .into_future()
                },
            ))
        }))
    }

    fn stream<D>(&self, uri: &str) -> Stream<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        struct State<D>
        where
            D: DeserializeOwned + 'static + Send,
        {
            uri: Url,
            items: Vec<D>,
            offset: u32,
            limit: u32,
            count: u32,
        }

        let instance = self.clone();

        Box::new(
            self.request::<_, List<D>>(Method::GET, &(self.host.clone() + uri), RequestBody::Empty)
                .map(move |(uri, list)| {
                    let mut state = State {
                        uri,
                        items: list.data,
                        offset: list.offset,
                        limit: list.limit,
                        count: list.total,
                    };
                    state.items.reverse();

                    stream::unfold::<_, _, Future<(D, State<D>)>, _>(state, move |mut state| {
                        match state.items.pop() {
                            Some(item) => {
                                state.count -= 1;
                                Some(Box::new(future::ok((item, state))))
                            }
                            _ => {
                                if state.count > 0 {
                                    let mut url = Url::parse(&state.uri.to_string())
                                        .expect("failed to parse uri");
                                    let mut map = BTreeMap::new();
                                    for (key, value) in url.query_pairs().into_owned() {
                                        map.insert(key, value);
                                    }
                                    map.insert(
                                        "_offset".to_string(),
                                        (state.offset + state.limit).to_string(),
                                    );
                                    url.query_pairs_mut().clear();
                                    url.query_pairs_mut().extend_pairs(map.iter());
                                    let next = Box::new(
                                        instance
                                            .request::<_, List<D>>(
                                                Method::GET,
                                                &url.to_string(),
                                                RequestBody::Empty,
                                            )
                                            .map(move |(uri, list)| {
                                                let mut state = State {
                                                    uri,
                                                    items: list.data,
                                                    limit: state.limit,
                                                    offset: state.offset + state.limit,
                                                    count: state.count - 1,
                                                };
                                                let item = state.items.remove(0);
                                                state.items.reverse();
                                                (item, state)
                                            }),
                                    )
                                        as Future<(D, State<D>)>;
                                    Some(next)
                                } else {
                                    None
                                }
                            }
                        }
                    })
                })
                .into_stream()
                .flatten(),
        )
    }

    fn get<D>(&self, uri: &str) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
    {
        self.request_entity(Method::GET, &(self.host.clone() + uri), RequestBody::Empty)
    }

    fn post<D, B>(&self, uri: &str, body: B) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
        B: Into<RequestBody>,
    {
        self.request_entity(
            Method::POST,
            &(self.host.clone() + uri),
            (body.into(), mime::APPLICATION_WWW_FORM_URLENCODED),
        )
    }

    fn post_form<M, D>(&self, uri: &str, data: M) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
        M: Into<Form>,
    {
        self.request_entity(
            Method::POST,
            &(self.host.clone() + uri),
            RequestBody::Form(data.into()),
        )
    }

    fn put<D, B>(&self, uri: &str, body: B) -> Future<D>
    where
        D: DeserializeOwned + 'static + Send,
        B: Into<RequestBody>,
    {
        self.request_entity(
            Method::PUT,
            &(self.host.clone() + uri),
            (body.into(), mime::APPLICATION_WWW_FORM_URLENCODED),
        )
    }

    fn delete<B>(&self, uri: &str, body: B) -> Future<()>
    where
        B: Into<RequestBody>,
    {
        Box::new(
            self.request_entity(
                Method::DELETE,
                &(self.host.clone() + uri),
                (body.into(), mime::APPLICATION_WWW_FORM_URLENCODED),
            )
            .or_else(|err| match err.kind() {
                error::ErrorKind::Json(_) => Ok(()),
                _ => Err(err),
            }),
        )
    }
}

pub(crate) enum RequestBody {
    Empty,
    Body(Body, Option<Mime>),
    Form(Form),
}

impl From<String> for RequestBody {
    fn from(s: String) -> RequestBody {
        RequestBody::Body(Body::from(s), None)
    }
}

impl From<(RequestBody, Mime)> for RequestBody {
    fn from(body: (RequestBody, Mime)) -> RequestBody {
        match body {
            (RequestBody::Body(body, _), mime) => RequestBody::Body(body, Some(mime)),
            (RequestBody::Empty, _) => RequestBody::Empty,
            _ => body.0,
        }
    }
}

/// Generic endpoint for sub-resources
pub struct Endpoint<Out>
where
    Out: DeserializeOwned + 'static,
{
    modio: Modio,
    path: String,
    phantom: PhantomData<Out>,
}

impl<Out> Endpoint<Out>
where
    Out: DeserializeOwned + 'static + Send,
{
    pub(crate) fn new(modio: Modio, path: String) -> Endpoint<Out> {
        Self {
            modio,
            path,
            phantom: PhantomData,
        }
    }

    pub fn list(&self) -> Future<List<Out>> {
        self.modio.get(&self.path)
    }

    pub fn iter(&self) -> Stream<Out> {
        self.modio.stream(&self.path)
    }

    pub fn add<T: AddOptions + QueryParams>(&self, options: &T) -> Future<ModioMessage> {
        let params = options.to_query_params();
        self.modio.post(&self.path, params)
    }

    pub fn delete<T: DeleteOptions + QueryParams>(&self, options: &T) -> Future<()> {
        let params = options.to_query_params();
        self.modio.delete(&self.path, params)
    }
}

filter_options! {
    /// Options used to filter event listings.
    ///
    /// # Filter parameters
    /// - id
    /// - game_id
    /// - mod_id
    /// - user_id
    /// - date_added
    /// - event_type
    ///
    /// # Sorting
    /// - id
    ///
    /// See the [modio docs](https://docs.mod.io/#events) for more information.
    ///
    /// By default this returns up to `100` items. You can limit the result using `limit` and
    /// `offset`.
    /// # Example
    /// ```
    /// use modio::filter::{Order, Operator};
    /// use modio::EventListOptions;
    /// use modio::mods::EventType;
    ///
    /// let mut opts = EventListOptions::new();
    /// opts.id(Operator::GreaterThan, 1024);
    /// opts.event_type(Operator::Equals, EventType::ModfileChanged);
    /// ```
    #[derive(Debug)]
    pub struct EventListOptions {
        Filters
        - id = "id";
        - game_id = "game_id";
        - mod_id = "mod_id";
        - user_id = "user_id";
        - date_added = "date_added";
        - event_type = "event_type";

        Sort
        - ID = "id";
    }
}

pub trait AddOptions {}
pub trait DeleteOptions {}

pub trait QueryParams {
    fn to_query_params(&self) -> String;
}
