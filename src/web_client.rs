use std::str::FromStr;

use futures_util::{FutureExt, TryFutureExt};

use super::*;

pub trait Asyncness {
    type Future<T: 'static>;
    fn map<T, O>(value: Self::Future<T>, fun: impl Transformer<T, O>) -> Self::Future<O>;
    fn flat_map<T, O>(
        value: Self::Future<T>,
        fun: impl Transformer<T, Self::Future<O>>,
    ) -> Self::Future<O>;
    fn and_then<T, O>(
        value: Self::Future<Result<T>>,
        fun: impl Transformer<T, Result<O>>,
    ) -> Self::Future<Result<O>>;
    fn flat_and_then<T, O>(
        value: Self::Future<Result<T>>,
        fun: impl Transformer<T, Self::Future<Result<O>>>,
    ) -> Self::Future<Result<O>>;
}

/// Web client agnostic implementation of a WebDAV client.
pub trait WebClient {
    type Asyncness: Asyncness;
    type Request: Request<Asyncness = Self::Asyncness, Response = Self::Response> + 'static;
    type Response: Response<Asyncness = Self::Asyncness> + 'static + Send;
    fn request(&self, method: &str, url: &str) -> Self::Request;
}

pub trait Request {
    // type Future<Output>;
    #[rustfmt::skip]
    /// Result of web client operations, should be either
    /// <code>[Pin]<[Box]<dyn [Future]<Output = [Result]<[MultiStatus]>>>></code>
    /// or <code>[Result]<[MultiStatus]></code>.
    type Asyncness: Asyncness;
    type Response: Response<Asyncness = Self::Asyncness>;
    #[must_use]
    fn header(self, key: &[u8], value: Vec<u8>) -> Self;
    #[must_use]
    fn send(
        self,
        body: Option<Vec<u8>>,
    ) -> <Self::Asyncness as Asyncness>::Future<Result<Self::Response>>;
}

pub trait Response: Sized {
    type Asyncness: Asyncness;
    // fn map_text<T, Fun>(self, fun: Fun) -> <Self::Asyncness as
    // Asyncness>::Future<Result<T>> where
    //     for<'a> Fun: Transformer<&'a str, Result<T>>;

    fn bytes(self) -> <Self::Asyncness as Asyncness>::Future<Result<Vec<u8>>>;
    fn text(self) -> <Self::Asyncness as Asyncness>::Future<Result<String>> {
        <Self::Asyncness>::map(self.bytes(), |b| {
            String::from_utf8(b?).map_err(Error::web_request)
        })
    }
}

// "trait alias"
pub trait Transformer<From, To>: Send + Sync + 'static + FnOnce(From) -> To {}

impl<Function, From, To> Transformer<From, To> for Function where
    Function: Send + Sync + 'static + FnOnce(From) -> To
{
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

type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;
pub struct Async;
impl Asyncness for Async {
    type Future<T: 'static> = BoxFuture<T>;

    fn map<T: 'static, O: 'static>(
        value: Self::Future<T>,
        fun: impl Transformer<T, O>,
    ) -> Self::Future<O> {
        value.map(fun).boxed()
    }

    fn flat_map<T: 'static, O: 'static>(
        value: Self::Future<T>,
        fun: impl Transformer<T, Self::Future<O>>,
    ) -> Self::Future<O> {
        value.map(fun).flatten().boxed()
    }

    fn and_then<T: 'static, O: 'static>(
        value: Self::Future<Result<T>>,
        fun: impl Transformer<T, Result<O>>,
    ) -> Self::Future<Result<O>> {
        value.map(|r| r.and_then(fun)).boxed()
    }

    fn flat_and_then<T: 'static, O: 'static>(
        value: Self::Future<Result<T>>,
        fun: impl Transformer<T, Self::Future<Result<O>>>,
    ) -> Self::Future<Result<O>> {
        value.and_then(fun).boxed()
    }
}
pub struct Blocking;
impl Asyncness for Blocking {
    type Future<T: 'static> = T;

    fn map<T: 'static, O: 'static>(
        value: Self::Future<T>,
        fun: impl Transformer<T, Self::Future<O>>,
    ) -> Self::Future<O> {
        fun(value)
    }

    fn flat_map<T: 'static, O: 'static>(
        value: Self::Future<T>,
        fun: impl Transformer<T, Self::Future<O>>,
    ) -> Self::Future<O> {
        fun(value)
    }

    fn and_then<T: 'static, O: 'static>(
        value: Self::Future<Result<T>>,
        fun: impl Transformer<T, Result<O>>,
    ) -> Self::Future<Result<O>> {
        value.and_then(fun)
    }

    fn flat_and_then<T: 'static, O: 'static>(
        value: Self::Future<Result<T>>,
        fun: impl Transformer<T, Self::Future<Result<O>>>,
    ) -> Self::Future<Result<O>> {
        value.and_then(fun)
    }
}

// #[cfg(nothing)]
mod reqwest {

    use futures_util::{FutureExt, TryFutureExt};
    use reqwest::{Client, RequestBuilder, Response};

    use super::{Async, BoxFuture, Error, FromStr, Result, WebClient};

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

        // fn map_text<T, Fun>(self, fun: Fun) -> BoxFuture<Result<T>>
        // where
        //     for<'a> Fun: Transformer<&'a str, Result<T>>,
        // {
        //     async move { fun(&self.text().await.map_err(Error::web_request)?)
        // }.boxed() }

        fn bytes(self) -> BoxFuture<Result<Vec<u8>>> {
            self.bytes()
                .map_ok(|b| b.to_vec())
                .map_err(Error::web_request)
                .boxed()
        }
    }
}

mod reqwest_blocking {

    use reqwest::blocking::{Client, RequestBuilder, Response};

    use super::{Blocking, Error, FromStr, Result, WebClient};

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
    }
}

mod ureq {
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
    }
}

pub use minreq::Minreq;
mod minreq {
    use ::minreq::{Request, Response};

    use super::{Blocking, Error, Result, WebClient, minreq, str};
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
    }
}
