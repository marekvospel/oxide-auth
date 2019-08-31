//! Bindings and utilities for creating an oauth endpoint with actix.
//!
//! Use the provided methods to use code grant methods in an asynchronous fashion, or use an
//! `AsActor<_>` to create an actor implementing endpoint functionality via messages.
#![warn(missing_docs)]

use actix::{MailboxError, Message};
use actix_web::{
    dev::{HttpResponseBuilder, Payload},
    error::BlockingError,
    http::{
        header::{
            HeaderMap, InvalidHeaderValue, InvalidHeaderValueBytes, AUTHORIZATION, CONTENT_TYPE,
            LOCATION, WWW_AUTHENTICATE,
        },
        HttpTryFrom, StatusCode,
    },
    web::Form,
    web::Query,
    FromRequest, HttpRequest, HttpResponse, Responder, ResponseError,
};
use futures::Future;
use oxide_auth::{
    endpoint::{Endpoint, NormalizedParameter, OAuthError, WebRequest, WebResponse},
    frontends::{
        dev::{Cow, QueryParameter},
        simple::endpoint::Error,
    },
};
use std::{error, fmt};
use url::Url;

mod operations;

pub use operations::{Authorize, Refresh, Resource, Token};

/// Describes an operation that can be performed in the presence of an `Endpoint`
///
/// This trait can be implemented by any type, but is very useful in Actor scenarios, where an
/// Actor can provide an endpoint to an operation sent as a message.
pub trait OxideOperation: Sized + 'static {
    /// The success-type produced by an OxideOperation
    type Item: 'static;

    /// The error type produced by an OxideOperation
    type Error: fmt::Debug + 'static;

    /// Performs the oxide operation with the provided endpoint
    fn run<E>(self, endpoint: E) -> Result<Self::Item, Self::Error>
    where
        E: Endpoint<OAuthRequest>,
        WebError: From<E::Error>;

    /// Turn an OxideOperation into a Message to send to an actor
    fn wrap(self) -> OxideMessage<Self> {
        OxideMessage(self)
    }
}

/// A message type to easily send `OxideOperation`s to an actor
pub struct OxideMessage<T>(T);

#[derive(Clone, Debug)]
/// Type implementing `WebRequest` as well as `FromRequest` for use in route handlers
///
/// This type consumes the body of the HttpRequest upon extraction, so be careful not to use it in
/// places you also expect an application payload
pub struct OAuthRequest {
    auth: Option<String>,
    query: Option<NormalizedParameter>,
    body: Option<NormalizedParameter>,
}

/// Type implementing `WebRequest` as well as `FromRequest` for use in guarding resources
///
/// This is useful over [OAuthRequest] since [OAuthResource] doesn't consume the body of the
/// request upon extraction
pub struct OAuthResource {
    auth: Option<String>,
}

#[derive(Clone, Debug)]
/// Type implementing `WebResponse` and `Responder` for use in route handlers
pub struct OAuthResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Option<String>,
}

#[derive(Debug)]
/// The error type for Oxide Auth operations
pub enum WebError {
    /// Errors occuring in Endpoint operations
    Endpoint(OAuthError),

    /// Errors occuring when producing Headers
    Header(InvalidHeaderValue),

    /// Errors occuring when producing Headers
    HeaderBytes(InvalidHeaderValueBytes),

    /// Errors with the request encoding
    Encoding,

    /// Request body could not be parsed as a form
    Form,

    /// Request query was absent or could not be parsed
    Query,

    /// Request was missing a body
    Body,

    /// The Authorization header was invalid
    Authorization,

    /// Processing part of the request was canceled
    Canceled,

    /// An actor's mailbox was full
    Mailbox,
}

impl OAuthRequest {
    /// Create a new OAuthRequest from an HttpRequest and Payload
    pub fn new(
        req: &HttpRequest,
        payload: &mut Payload,
    ) -> impl Future<Item = Self, Error = WebError> {
        let query_res = Query::extract(req);
        let form_fut = Form::from_request(&req, payload);

        let req = req.clone();

        form_fut.then(move |form_res| {
            let body = form_res
                .ok()
                .map(|b: Form<NormalizedParameter>| b.into_inner());
            let query = query_res
                .ok()
                .map(|q: Query<NormalizedParameter>| q.into_inner());

            let mut all_auth = req.headers().get_all(AUTHORIZATION);
            let optional = all_auth.next();

            let auth = if let Some(_) = all_auth.next() {
                return Err(WebError::Authorization);
            } else {
                optional.and_then(|hv| hv.to_str().ok().map(str::to_owned))
            };

            Ok(OAuthRequest { auth, query, body })
        })
    }

    /// Fetch the authorization header from the request
    pub fn authorization_header(&self) -> Option<&str> {
        self.auth.as_ref().map(|s| s.as_str())
    }

    /// Fetch the query for this request
    pub fn query(&self) -> Option<&NormalizedParameter> {
        self.query.as_ref()
    }

    /// Fetch the query mutably
    pub fn query_mut(&mut self) -> Option<&mut NormalizedParameter> {
        self.query.as_mut()
    }

    /// Fetch the body of the request
    pub fn body(&self) -> Option<&NormalizedParameter> {
        self.body.as_ref()
    }
}

impl OAuthResource {
    /// Create a new OAuthResource from an HttpRequest
    pub fn new(req: &HttpRequest) -> Result<Self, WebError> {
        let mut all_auth = req.headers().get_all(AUTHORIZATION);
        let optional = all_auth.next();

        let auth = if let Some(_) = all_auth.next() {
            return Err(WebError::Authorization);
        } else {
            optional.and_then(|hv| hv.to_str().ok().map(str::to_owned))
        };

        Ok(OAuthResource { auth })
    }

    /// Turn this OAuthResource into an OAuthRequest for processing
    pub fn into_request(self) -> OAuthRequest {
        OAuthRequest {
            query: None,
            body: None,
            auth: self.auth,
        }
    }
}

impl OAuthResponse {
    /// Create a simple response with no body and a '200 OK' HTTP Status
    pub fn ok() -> Self {
        OAuthResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: None,
        }
    }

    /// Set the `ContentType` header on a response
    pub fn content_type(mut self, content_type: &str) -> Result<Self, WebError> {
        self.headers
            .insert(CONTENT_TYPE, HttpTryFrom::try_from(content_type)?);
        Ok(self)
    }

    /// Set the bodyfor the response
    pub fn body(mut self, body: &str) -> Self {
        self.body = Some(body.to_owned());
        self
    }
}

impl<T> OxideMessage<T> {
    /// Produce an OxideOperation from a wrapping OxideMessage
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl WebRequest for OAuthRequest {
    type Error = WebError;
    type Response = OAuthResponse;

    fn query(&mut self) -> Result<Cow<dyn QueryParameter + 'static>, Self::Error> {
        self.query
            .as_ref()
            .map(|q| Cow::Borrowed(q as &dyn QueryParameter))
            .ok_or(WebError::Query)
    }

    fn urlbody(&mut self) -> Result<Cow<dyn QueryParameter + 'static>, Self::Error> {
        self.body
            .as_ref()
            .map(|b| Cow::Borrowed(b as &dyn QueryParameter))
            .ok_or(WebError::Body)
    }

    fn authheader(&mut self) -> Result<Option<Cow<str>>, Self::Error> {
        Ok(self.auth.as_ref().map(String::as_str).map(Cow::Borrowed))
    }
}

impl WebResponse for OAuthResponse {
    type Error = WebError;

    fn ok(&mut self) -> Result<(), Self::Error> {
        self.status = StatusCode::OK;
        Ok(())
    }

    fn redirect(&mut self, url: Url) -> Result<(), Self::Error> {
        self.status = StatusCode::FOUND;
        self.headers
            .insert(LOCATION, HttpTryFrom::try_from(url.into_string())?);
        Ok(())
    }

    fn client_error(&mut self) -> Result<(), Self::Error> {
        self.status = StatusCode::BAD_REQUEST;
        Ok(())
    }

    fn unauthorized(&mut self, kind: &str) -> Result<(), Self::Error> {
        self.status = StatusCode::UNAUTHORIZED;
        self.headers
            .insert(WWW_AUTHENTICATE, HttpTryFrom::try_from(kind)?);
        Ok(())
    }

    fn body_text(&mut self, text: &str) -> Result<(), Self::Error> {
        self.body = Some(text.to_owned());
        self.headers
            .insert(CONTENT_TYPE, HttpTryFrom::try_from("text/plain")?);
        Ok(())
    }

    fn body_json(&mut self, json: &str) -> Result<(), Self::Error> {
        self.body = Some(json.to_owned());
        self.headers
            .insert(CONTENT_TYPE, HttpTryFrom::try_from("application/json")?);
        Ok(())
    }
}

impl<T> Message for OxideMessage<T>
where
    T: OxideOperation + 'static,
    T::Item: 'static,
    T::Error: 'static,
{
    type Result = Result<T::Item, T::Error>;
}

impl FromRequest for OAuthRequest {
    type Error = WebError;
    type Future = Box<dyn Future<Item = Self, Error = Self::Error>>;
    type Config = ();

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        Box::new(Self::new(req, payload))
    }
}

impl FromRequest for OAuthResource {
    type Error = WebError;
    type Future = Result<Self, Self::Error>;
    type Config = ();

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        Self::new(req)
    }
}

impl Responder for OAuthResponse {
    type Error = WebError;
    type Future = Result<HttpResponse, Self::Error>;

    fn respond_to(self, _: &HttpRequest) -> Self::Future {
        let mut builder = HttpResponseBuilder::new(self.status);
        for (k, v) in self.headers.into_iter() {
            builder.header(k, v.to_owned());
        }

        if let Some(body) = self.body {
            Ok(builder.body(body))
        } else {
            Ok(builder.finish())
        }
    }
}

impl From<OAuthResource> for OAuthRequest {
    fn from(o: OAuthResource) -> Self {
        o.into_request()
    }
}

impl Default for OAuthResponse {
    fn default() -> Self {
        OAuthResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: None,
        }
    }
}

impl From<Error<OAuthRequest>> for WebError {
    fn from(e: Error<OAuthRequest>) -> Self {
        match e {
            Error::Web(e) => e,
            Error::OAuth(e) => e.into(),
        }
    }
}

impl From<InvalidHeaderValue> for WebError {
    fn from(e: InvalidHeaderValue) -> Self {
        WebError::Header(e)
    }
}

impl From<InvalidHeaderValueBytes> for WebError {
    fn from(e: InvalidHeaderValueBytes) -> Self {
        WebError::HeaderBytes(e)
    }
}

impl<E> From<BlockingError<E>> for WebError
where
    E: Into<WebError> + fmt::Debug,
{
    fn from(e: BlockingError<E>) -> Self {
        match e {
            BlockingError::Canceled => WebError::Canceled,
            BlockingError::Error(e) => e.into(),
        }
    }
}

impl From<MailboxError> for WebError {
    fn from(e: MailboxError) -> Self {
        match e {
            MailboxError::Closed => WebError::Mailbox,
            MailboxError::Timeout => WebError::Canceled,
        }
    }
}

impl From<OAuthError> for WebError {
    fn from(e: OAuthError) -> Self {
        WebError::Endpoint(e)
    }
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            WebError::Endpoint(ref e) => write!(f, "Endpoint, {}", e),
            WebError::Header(ref e) => write!(f, "Couldn't set header, {}", e),
            WebError::HeaderBytes(ref e) => write!(f, "Couldn't set header, {}", e),
            WebError::Encoding => write!(f, "Error decoding request"),
            WebError::Form => write!(f, "Request is not a form"),
            WebError::Query => write!(f, "No query present"),
            WebError::Body => write!(f, "No body present"),
            WebError::Authorization => write!(f, "Request has invalid Authorization headers"),
            WebError::Canceled => write!(f, "Operation canceled"),
            WebError::Mailbox => write!(f, "An actor's mailbox was full"),
        }
    }
}

impl error::Error for WebError {
    fn description(&self) -> &str {
        match *self {
            WebError::Endpoint(ref e) => e.description(),
            WebError::Header(ref e) => e.description(),
            WebError::HeaderBytes(ref e) => e.description(),
            WebError::Encoding => "Error decoding request",
            WebError::Form => "Request is not a form",
            WebError::Query => "No query present",
            WebError::Body => "No body present",
            WebError::Authorization => "Request has invalid Authorization headers",
            WebError::Canceled => "Operation canceled",
            WebError::Mailbox => "An actor's mailbox was full",
        }
    }

    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            WebError::Endpoint(ref e) => e.source(),
            WebError::Header(ref e) => e.source(),
            WebError::HeaderBytes(ref e) => e.source(),
            WebError::Encoding
            | WebError::Form
            | WebError::Authorization
            | WebError::Query
            | WebError::Body
            | WebError::Canceled
            | WebError::Mailbox => None,
        }
    }
}

impl ResponseError for WebError {
    // Default to 500 for now
}
