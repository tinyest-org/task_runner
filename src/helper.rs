use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::{
    error,
    http::header::{Header, TryIntoHeaderValue},
};
use std::fmt;

#[derive(Debug)]
pub struct Requester(String);

impl Header for Requester {
    fn name() -> HeaderName {
        HeaderName::from_static("requester")
    }

    fn parse<M>(msg: &M) -> Result<Self, error::ParseError>
    where
        M: actix_web::HttpMessage,
    {
        if let Some(header_value) = msg.headers().get(HeaderName::from_static("requester")) {
            header_value
                .to_str()
                .map(|s| Requester(s.to_owned()))
                .map_err(|_| error::ParseError::Header)
        } else {
            Err(error::ParseError::Header)
        }
    }
}

impl fmt::Display for Requester {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryIntoHeaderValue for Requester {
    type Error = actix_web::http::header::InvalidHeaderValue;

    fn try_into_value(self) -> Result<HeaderValue, Self::Error> {
        HeaderValue::from_str(&self.0)
    }
}
