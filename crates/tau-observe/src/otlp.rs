//! OTLP endpoint configuration.
//!
//! Feature-gated: this module only compiles when feature `otlp` is on.

#![cfg(feature = "otlp")]

use std::collections::HashMap;

/// Connection parameters for an OTLP/gRPC collector.
#[derive(Debug, Clone)]
pub struct OtlpEndpoint {
    /// e.g. `"https://otel.example.com:4317"`.
    pub endpoint: String,
    /// Extra gRPC metadata headers (auth bearer tokens, tenant ids, …).
    /// Maps to `tonic::metadata::MetadataMap` at install time.
    pub headers: HashMap<String, String>,
}

impl OtlpEndpoint {
    /// Read endpoint + headers from the standard OTel env vars.
    /// Returns `None` if neither `OTEL_EXPORTER_OTLP_ENDPOINT` nor
    /// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is set.
    pub fn from_env() -> Option<Self> {
        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
            .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT"))
            .ok()?;
        let headers = std::env::var("OTEL_EXPORTER_OTLP_HEADERS")
            .ok()
            .map(parse_headers)
            .unwrap_or_default();
        Some(Self { endpoint, headers })
    }
}

fn parse_headers(raw: String) -> HashMap<String, String> {
    raw.split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?.trim().to_string();
            let val = parts.next()?.trim().to_string();
            if key.is_empty() { None } else { Some((key, val)) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_headers_splits_on_comma_and_equals() {
        let h = parse_headers("authorization=Bearer abc,tenant=acme".to_string());
        assert_eq!(h.get("authorization").map(String::as_str), Some("Bearer abc"));
        assert_eq!(h.get("tenant").map(String::as_str), Some("acme"));
    }

    #[test]
    fn parse_headers_ignores_malformed_pairs() {
        let h = parse_headers("good=1,bad".to_string());
        assert_eq!(h.len(), 1);
        assert_eq!(h.get("good").map(String::as_str), Some("1"));
    }
}
