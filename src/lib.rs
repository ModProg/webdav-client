use std::fmt::Write;
use std::pin::Pin;

use futures_util::future::FutureExt;
pub mod webdav_types;

mod web_client;
use std::future::Future;

use reqwest_dav::{Auth, Depth};
pub use web_client::*;

pub struct Error;
type Result<T, E = Error> = std::result::Result<T, E>;

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

pub trait Client {
    type Result;
    type RequestBuilder: RequestBuilder<Result = Self::Result>;
    fn prop_find(
        &self,
        url: &str,
        authentication: Auth,
        depth: Depth,
        fields: &[&str],
        name_spaces: &[(&str, &str)],
    ) -> Self::Result {
        let Auth::Basic(username, password) = authentication else {
            todo!();
        };
        let auth = basic_auth(&username, Some(&password));
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
        self.request(b"PROPFIND", url)
            .header(b"authorization", auth)
            .header(b"depth", match depth {
                Depth::Number(n) => n.to_string().into_bytes(),
                Depth::Infinity => b"infinity".to_vec(),
            })
            .body(body.into_bytes())
            .send()
    }

    fn request(&self, method: &[u8], url: &str) -> Self::RequestBuilder;
}

pub trait RequestBuilder {
    type Result;
    #[must_use]
    fn header(self, name: &[u8], value: Vec<u8>) -> Self;
    #[must_use]
    fn body(self, body: Vec<u8>) -> Self;
    #[must_use]
    fn send(self) -> Self::Result;
}

impl RequestBuilder for reqwest::RequestBuilder {
    type Result = Pin<Box<dyn Future<Output = Result<webdav_types::MultiStatus>>>>;

    fn header(self, name: &[u8], value: Vec<u8>) -> Self {
        self.header(name, value)
    }

    fn body(self, body: Vec<u8>) -> Self {
        self.body(body)
    }

    fn send(self) -> Self::Result {
        self.send()
            .map(async |response| {
                if let Ok(response) = response {
                    let response = response.text().await.map_err(|_| Error)?;
                    quick_xml::de::from_str(&response).map_err(|_| Error)
                } else {
                    Err(Error)
                }
            })
            .flatten()
            .boxed()
    }
}

impl Client for reqwest::Client {
    type RequestBuilder = reqwest_dav::re_exports::reqwest::RequestBuilder;
    type Result = Pin<Box<dyn Future<Output = Result<webdav_types::MultiStatus>>>>;

    fn request(&self, method: &[u8], url: &str) -> Self::RequestBuilder {
        self.request(reqwest::Method::from_bytes(method).unwrap(), url)
    }
}
