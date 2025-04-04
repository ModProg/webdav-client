use std::future::ready;

#[cfg(all(feature = "batteries", feature = "attohttpc"))]
pub use attohttpc;
#[cfg(feature = "async")]
use futures_util::{FutureExt, TryFutureExt};
#[cfg(all(feature = "batteries", feature = "minreq"))]
pub use minreq;
#[cfg(all(feature = "batteries", feature = "reqwest"))]
pub use reqwest;
#[cfg(all(feature = "batteries", feature = "ureq"))]
pub use ureq;

use super::*;

pub trait Asyncness {
    type Future<T: 'static>;
    fn ready<T: Send>(value: T) -> Self::Future<T>;
    fn map<T, O>(
        value: Self::Future<T>,
        fun: impl FnOnce(T) -> O + Send + 'static,
    ) -> Self::Future<O>;
    fn flat_map<T, O>(
        value: Self::Future<T>,
        fun: impl FnOnce(T) -> Self::Future<O> + Send + 'static,
    ) -> Self::Future<O>;
    fn and_then<T, O>(
        value: Self::Future<Result<T>>,
        fun: impl FnOnce(T) -> Result<O> + Send + 'static,
    ) -> Self::Future<Result<O>>;
    fn flat_and_then<T, O>(
        value: Self::Future<Result<T>>,
        fun: impl FnOnce(T) -> Self::Future<Result<O>> + Send + 'static,
    ) -> Self::Future<Result<O>>;
}

/// Web client agnostic implementation of a WebDAV client.
pub trait WebClient {
    type Asyncness: Asyncness;
    type Request: Request<Asyncness = Self::Asyncness, Response = Self::Response> + 'static;
    type Response: Response<Asyncness = Self::Asyncness> + 'static + Send;
    fn request(&self, method: &str, url: &str) -> Self::Request;
}

pub trait Request: Sized {
    type Asyncness: Asyncness;
    type Response: Response<Asyncness = Self::Asyncness>;
    #[must_use]
    fn header(self, key: &[u8], value: Vec<u8>) -> Self;
    #[must_use]
    #[deprecated = "probably use `send_ok` unless you handle HTTP status codes"]
    fn send(
        self,
        body: Option<Vec<u8>>,
    ) -> <Self::Asyncness as Asyncness>::Future<Result<Self::Response>>;
    #[must_use]
    fn send_ok(
        self,
        body: Option<Vec<u8>>,
    ) -> <Self::Asyncness as Asyncness>::Future<Result<Self::Response>> {
        #![allow(deprecated)]
        Self::Asyncness::flat_and_then(self.send(body), Response::error_on_status_code)
    }
}

pub trait Response: Sized + Send {
    type Asyncness: Asyncness;

    fn bytes(self) -> <Self::Asyncness as Asyncness>::Future<Result<Vec<u8>>>;
    fn text(self) -> <Self::Asyncness as Asyncness>::Future<Result<String>> {
        <Self::Asyncness>::map(self.bytes(), |b| {
            String::from_utf8(b?).map_err(Error::web_request)
        })
    }
    fn status(&self) -> u16;
    fn error_on_status_code(self) -> <Self::Asyncness as Asyncness>::Future<Result<Self>> {
        let status = self.status();
        if (200..300).contains(&status) {
            <Self::Asyncness>::ready(Ok(self))
        } else {
            Self::Asyncness::map(self.text(), move |text| {
                Err(Error::ErrorStatus {
                    status,
                    text: text.ok(),
                })
            })
        }
    }
}

impl<T: WebClient> WebClient for super::Client<T> {
    type Asyncness = T::Asyncness;
    type Request = T::Request;
    type Response = T::Response;

    fn request(&self, method: &str, url: &str) -> Self::Request {
        let request = self.web_client.request(method, url);
        if let Auth::Basic { username, password } = &self.authentication {
            let auth = basic_auth(username, password.as_deref());
            request.header(b"authorization", auth)
        } else {
            request
        }
    }
}

#[cfg(feature = "async")]
type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;
#[cfg(feature = "async")]
pub struct Async;
#[cfg(feature = "async")]
impl Asyncness for Async {
    type Future<T: 'static> = BoxFuture<T>;

    fn ready<T: Send + 'static>(value: T) -> Self::Future<T> {
        ready(value).boxed()
    }

    fn map<T: 'static, O: 'static>(
        value: Self::Future<T>,
        fun: impl FnOnce(T) -> O + Send + 'static,
    ) -> Self::Future<O> {
        value.map(fun).boxed()
    }

    fn flat_map<T: 'static, O: 'static>(
        value: Self::Future<T>,
        fun: impl FnOnce(T) -> Self::Future<O> + Send + 'static,
    ) -> Self::Future<O> {
        value.map(fun).flatten().boxed()
    }

    fn and_then<T: 'static, O: 'static>(
        value: Self::Future<Result<T>>,
        fun: impl FnOnce(T) -> Result<O> + Send + 'static,
    ) -> Self::Future<Result<O>> {
        value.map(|r| r.and_then(fun)).boxed()
    }

    fn flat_and_then<T: 'static, O: 'static>(
        value: Self::Future<Result<T>>,
        fun: impl FnOnce(T) -> Self::Future<Result<O>> + Send + 'static,
    ) -> Self::Future<Result<O>> {
        value.and_then(fun).boxed()
    }
}

pub struct Blocking;
impl Asyncness for Blocking {
    type Future<T: 'static> = T;

    fn ready<T: Send>(value: T) -> T {
        value
    }

    fn map<T, O>(value: T, fun: impl FnOnce(T) -> O) -> O {
        fun(value)
    }

    fn flat_map<T, O>(value: T, fun: impl FnOnce(T) -> O) -> O {
        fun(value)
    }

    fn and_then<T, O>(value: Result<T>, fun: impl FnOnce(T) -> Result<O>) -> Result<O> {
        value.and_then(fun)
    }

    fn flat_and_then<T, O>(value: Result<T>, fun: impl FnOnce(T) -> Result<O>) -> Result<O> {
        value.and_then(fun)
    }
}

#[cfg(feature = "reqwest")]
mod reqwest_impl {

    use std::str::FromStr;

    use futures_util::{FutureExt, TryFutureExt};
    use reqwest::{Client, RequestBuilder, Response};

    use super::{Async, BoxFuture, Error, Result, WebClient};

    impl WebClient for Client {
        type Asyncness = Async;
        type Request = RequestBuilder;
        type Response = Response;

        fn request(&self, method: &str, url: &str) -> Self::Request {
            self.request(reqwest::Method::from_str(method).unwrap(), url)
        }
    }

    impl super::Request for RequestBuilder {
        type Asyncness = Async;
        type Response = Response;

        fn header(self, key: &[u8], value: Vec<u8>) -> Self {
            self.header(key, value)
        }

        fn send(self, body: Option<Vec<u8>>) -> BoxFuture<Result<Response>> {
            if let Some(body) = body {
                self.body(body)
            } else {
                self
            }
            .send()
            .map_err(Error::web_request)
            .boxed()
        }
    }

    impl super::Response for Response {
        type Asyncness = Async;

        fn bytes(self) -> BoxFuture<Result<Vec<u8>>> {
            self.bytes()
                .map_ok(|b| b.to_vec())
                .map_err(Error::web_request)
                .boxed()
        }

        fn status(&self) -> u16 {
            self.status().as_u16()
        }
    }
}

#[cfg(feature = "reqwest-blocking")]
mod reqwest_blocking_impl {

    use std::str::FromStr;

    use reqwest::blocking::{Client, RequestBuilder, Response};

    use super::{Blocking, Error, Result, WebClient};

    impl WebClient for Client {
        type Asyncness = Blocking;
        type Request = RequestBuilder;
        type Response = Response;

        fn request(&self, method: &str, url: &str) -> Self::Request {
            self.request(reqwest::Method::from_str(method).unwrap(), url)
        }
    }

    impl super::Request for RequestBuilder {
        type Asyncness = Blocking;
        type Response = Response;

        fn header(self, key: &[u8], value: Vec<u8>) -> Self {
            self.header(key, value)
        }

        fn send(self, body: Option<Vec<u8>>) -> Result<Response> {
            if let Some(body) = body {
                self.body(body)
            } else {
                self
            }
            .send()
            .map_err(Error::web_request)
        }
    }

    impl super::Response for Response {
        type Asyncness = Blocking;

        fn bytes(self) -> Result<Vec<u8>> {
            self.bytes().map(|b| b.to_vec()).map_err(Error::web_request)
        }

        fn status(&self) -> u16 {
            self.status().as_u16()
        }
    }
}

#[cfg(feature = "ureq")]
mod ureq_impl {
    use http::response::Response;
    use ureq::Body;

    use super::{Blocking, Error, Request, Result, WebClient};

    impl WebClient for ureq::Agent {
        type Asyncness = Blocking;
        type Request = (Self, ureq::http::request::Builder);
        type Response = Response<Body>;

        fn request(&self, method: &str, url: &str) -> Self::Request {
            (
                self.clone(),
                ureq::http::request::Builder::new().method(method).uri(url),
            )
        }
    }

    impl Request for (ureq::Agent, ureq::http::request::Builder) {
        type Asyncness = Blocking;
        type Response = Response<Body>;

        fn header(self, key: &[u8], value: Vec<u8>) -> Self {
            (self.0, self.1.header(key, value))
        }

        fn send(self, body: Option<Vec<u8>>) -> Result<Self::Response> {
            if let Some(body) = body {
                self.0
                    .run(
                        self.0
                            .configure_request(self.1.body(body).map_err(Error::web_request)?)
                            .allow_non_standard_methods(true)
                            .build(),
                    )
                    .map_err(Error::web_request)
            } else {
                self.0
                    .run(
                        self.0
                            .configure_request(self.1.body(()).map_err(Error::web_request)?)
                            .allow_non_standard_methods(true)
                            .build(),
                    )
                    .map_err(Error::web_request)
            }
        }
    }

    impl super::Response for Response<Body> {
        type Asyncness = Blocking;

        fn bytes(self) -> <Self::Asyncness as super::Asyncness>::Future<Result<Vec<u8>>> {
            self.into_body().read_to_vec().map_err(Error::web_request)
        }

        fn status(&self) -> u16 {
            self.status().as_u16()
        }
    }
}

#[cfg(feature = "minreq")]
pub use minreq_impl::Minreq;
#[cfg(feature = "minreq")]
mod minreq_impl {
    use minreq::{Request, Response};

    use super::{Blocking, Error, Result, WebClient, str};
    pub struct Minreq;
    impl WebClient for Minreq {
        type Asyncness = Blocking;
        type Request = Request;
        type Response = Response;

        fn request(&self, method: &str, url: &str) -> Request {
            minreq::Request::new(::minreq::Method::Custom(method.to_owned()), url)
        }
    }

    impl super::Request for Request {
        type Asyncness = Blocking;
        type Response = Response;

        fn header(self, key: &[u8], value: Vec<u8>) -> Self {
            self.with_header(
                str::from_utf8(key).unwrap(),
                String::from_utf8(value).unwrap(),
            )
        }

        fn send(self, body: Option<Vec<u8>>) -> Result<Response> {
            if let Some(body) = body {
                self.with_body(body)
            } else {
                self
            }
            .send()
            .map_err(Error::web_request)
        }
    }

    impl super::Response for Response {
        type Asyncness = Blocking;

        fn bytes(self) -> Result<Vec<u8>> {
            Ok(self.into_bytes())
        }

        fn status(&self) -> u16 {
            use intentional::CastInto;
            self.status_code.cast_into()
        }
    }
}

#[cfg(feature = "attohttpc")]
pub use attohttpc_impl::Attohttpc;
#[cfg(feature = "attohttpc")]
mod attohttpc_impl {
    use attohttpc::body::Bytes;
    use attohttpc::{RequestBuilder, Response};
    use http::Method;
    use intentional::Assert;

    /// Marker struct used until <https://github.com/sbstp/attohttpc/issues/188> is resolved.
    pub struct Attohttpc;

    use super::{Blocking, Error, Result, WebClient, str};
    impl WebClient for Attohttpc {
        type Asyncness = Blocking;
        type Request = RequestBuilder;
        type Response = Response;

        fn request(&self, method: &str, url: &str) -> RequestBuilder {
            RequestBuilder::new(Method::from_bytes(method.as_bytes()).assert_expected(), url)
        }
    }

    impl super::Request for RequestBuilder {
        type Asyncness = Blocking;
        type Response = Response;

        fn header(self, key: &[u8], value: Vec<u8>) -> Self {
            self.header(http::HeaderName::from_bytes(key).assert_expected(), value)
        }

        fn send(self, body: Option<Vec<u8>>) -> Result<Response> {
            if let Some(body) = body {
                self.body(Bytes(body)).send()
            } else {
                self.send()
            }
            .map_err(Error::web_request)
        }
    }

    impl super::Response for Response {
        type Asyncness = Blocking;

        fn bytes(self) -> Result<Vec<u8>> {
            self.bytes().map_err(Error::web_request)
        }

        fn status(&self) -> u16 {
            self.status().as_u16()
        }
    }
}
