use std::sync::LazyLock;

use axum::http::HeaderValue;
use axum_extra::headers::{self, Header};

static HEADER_REAL_IP_NAME: LazyLock<axum::http::HeaderName> =
    LazyLock::new(|| "X-Real-IP".parse().unwrap());

pub struct RealIP(String);

impl Header for RealIP {
    fn name() -> &'static axum::http::HeaderName {
        &HEADER_REAL_IP_NAME
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, axum_extra::headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i axum::http::HeaderValue>,
    {
        let value = values.next().ok_or_else(headers::Error::invalid)?;
        value
            .to_str()
            .map(|s| Self(s.to_string()))
            .map_err(|_| headers::Error::invalid())
    }

    fn encode<E: Extend<axum::http::HeaderValue>>(&self, values: &mut E) {
        let s =
            HeaderValue::from_str(&self.0).unwrap_or_else(|_| HeaderValue::from_static("ERROR"));
        values.extend(std::iter::once(s))
    }
}

impl RealIP {
    pub fn into_inner(self) -> String {
        self.0
    }
}
