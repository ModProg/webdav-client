//! Web client library agnostic implementation of WebDAV.
use std::fmt::{Display, Write};
use std::str;

use derive_more::{Display, Error, From};

pub mod webdav_types;
use webdav_types::MultiStatus;

mod web_client;
pub use web_client::*;

#[derive(Clone, derive_more::Debug)]
/// Select authentication method.
pub enum Auth {
    /// No authentication.
    None,
    /// [Basic Auth](https://developer.mozilla.org/en-US/docs/Web/HTTP/Authentication#basic_authentication_scheme).
    Basic {
        /// Username.
        username: String,
        /// Password, optional.
        #[debug(skip)]
        password: Option<String>,
    },
    // TODO Digest(),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
/// Select depth [`Client::prop_find`].
pub enum Depth {
    /// Specific depth.
    Some(u32),
    /// Infinit depth.
    Infinity,
}

#[derive(Display, Debug, Error, From)]
/// Error returned by [`Client`].
pub enum Error {
    #[display("Web request error: {_0}")]
    #[from(skip)]
    /// Error caused in web client used.
    WebRequest(Box<dyn std::error::Error + Send + Sync>),
    /// Error caused in parsing the response.
    Parsing(quick_xml::DeError),
    #[display("Non 200 status code {status} {}", text.as_deref().unwrap_or_default())]
    ErrorStatus { status: u16, text: Option<String> },
}

impl Error {
    #[must_use]
    pub fn web_request(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::WebRequest(Box::new(source))
    }

    #[must_use]
    pub fn is_404(&self) -> bool {
        matches!(self, Self::ErrorStatus { status: 404, .. })
    }
}

/// Result returned by [`Client`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn basic_auth(username: &str, password: Option<&str>) -> Vec<u8> {
    use std::io::Write;

    use base64::prelude::BASE64_STANDARD;
    use base64::write::EncoderWriter;

    let mut buf = b"Basic ".to_vec();
    {
        let mut encoder = EncoderWriter::new(&mut buf, &BASE64_STANDARD);
        let _ = write!(encoder, "{username}:");
        if let Some(password) = password {
            let _ = write!(encoder, "{password}");
        }
    }
    buf
}

#[derive(Clone, Debug)]
pub struct Client<T> {
    pub web_client: T,
    pub authentication: Auth,
}

impl<T> Client<T> {
    pub fn new(web_client: T) -> Self {
        Self {
            web_client,
            authentication: Auth::None,
        }
    }

    pub fn authenticated(web_client: T, authentication: Auth) -> Self {
        Self {
            web_client,
            authentication,
        }
    }
}

impl<T: WebClient<Asyncness = A>, A: Asyncness> Client<T> {
    pub fn prop_find(
        &self,
        url: impl AsRef<str>,
        depth: Depth,
        fields: impl IntoIterator<Item = impl Display>,
        name_spaces: impl IntoIterator<Item = (impl Display, impl Display)>,
    ) -> A::Future<Result<MultiStatus>> {
        let mut body = String::new();
        write!(body, r#"<?xml version="1.0"?><d:propfind"#).unwrap();
        for (name, space) in name_spaces {
            write!(body, r#" xmlns:{name}="{space}""#).unwrap();
        }
        write!(body, "><d:prop>").unwrap();
        for name in fields {
            write!(body, "<{name}/>").unwrap();
        }
        write!(body, "</d:prop></d:propfind>").unwrap();
        // todo!()
        let response = self
            .request("PROPFIND", url.as_ref())
            .header(b"depth", match depth {
                Depth::Some(n) => n.to_string().into_bytes(),
                Depth::Infinity => b"infinity".to_vec(),
            })
            .send_ok(Some(body.into_bytes()));
        let response = A::flat_and_then(response, Response::text);
        A::and_then(response, |s| {
            quick_xml::de::from_str(&s).map_err(Error::Parsing)
        })
    }

    pub fn get(&self, url: impl AsRef<str>) -> A::Future<Result<Vec<u8>>> {
        A::flat_and_then(self.get_raw(url), web_client::Response::bytes)
    }

    pub fn get_raw(&self, url: impl AsRef<str>) -> A::Future<Result<T::Response>> {
        self.request("GET", url.as_ref()).send_ok(None)
    }

    // pub fn put(&self, url: impl AsRef<str>, data: Vec<u8>) -> <T::Request as
    // Request>::Result<()> {     self.request("GET", url.as_ref()).send(None);
    // }

    pub fn put_raw(&self, url: impl AsRef<str>) -> T::Request {
        self.request("PUT", url.as_ref())
    }
}
