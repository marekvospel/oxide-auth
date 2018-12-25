//! Bindings and utilities for creating an oauth endpoint with actix.
extern crate actix;
extern crate actix_web;
extern crate futures;
extern crate serde_urlencoded;

mod future_endpoint;
mod endpoint;
pub mod message;
pub mod request;
#[cfg(test)]
mod tests;

use std::fmt;
use std::error;

use self::actix_web::{HttpRequest, HttpResponse};
use self::actix_web::ResponseError;
use code_grant::endpoint::OAuthError;

// pub use self::endpoint::CodeGrantEndpoint;
pub use self::request::OAuthFuture;
pub use self::request::OAuthRequest;
pub use self::request::OAuthResponse;
pub use code_grant::endpoint::{AuthorizationFlow, AccessTokenFlow, ResourceFlow, PreGrant, OwnerConsent, OwnerSolicitor};

pub use self::future_endpoint::{ResourceProtection, access_token, authorization, resource};

/// Bundles all oauth related methods under a single type.
pub trait OAuth {
    /// Convert an http request to an oauth request which provides all possible sub types.
    fn oauth2(self) -> OAuthFuture;
}

/// Newtype wrapper around a primitive, transforming it into an actor.
pub struct AsActor<P>(pub P);

/// Newtype struct wrapper around an error.
///
/// Implements the `actix_web::ResponseError` trait so it can be used as an error in a route.
#[derive(Debug)]
pub struct OAuthFailure(pub OAuthError);

impl<'a, State> OAuth for &'a HttpRequest<State> {
    fn oauth2(self) -> OAuthFuture {
        OAuthFuture::new(self)
    }
}

impl fmt::Display for OAuthFailure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            OAuthError::DenySilently => f.write_str("Suspicious request may be an attack"),
            OAuthError::PrimitiveError => f.write_str("Server component failed during OAuth flow"),
            OAuthError::InvalidRequest => f.write_str("Request was invalid"),
        }
    }
}

impl error::Error for OAuthFailure { }

impl ResponseError for OAuthFailure {
    fn error_response(&self) -> HttpResponse {
        match self.0 {
            OAuthError::DenySilently => HttpResponse::BadRequest().finish(),
            OAuthError::PrimitiveError => HttpResponse::InternalServerError().finish(),
            OAuthError::InvalidRequest => HttpResponse::BadRequest().finish(),
        }
    }
}
