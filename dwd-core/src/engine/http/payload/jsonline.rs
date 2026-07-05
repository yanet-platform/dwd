use core::{error::Error, str::FromStr};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use bytes::Bytes;
use http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, Uri};
use http_body_util::Empty;
use serde::{
    de::{self, Unexpected},
    Deserialize, Deserializer,
};

/// Yandex.Tank JSON ammo format.
///
/// Pay attention to special header "Host" defined outside of "Headers"
/// dictionary.
///
/// Host inside "Headers" section will be silently ignored.
#[derive(Debug, Deserialize)]
pub struct JsonLineRecord {
    #[serde(deserialize_with = "deserialize_http_uri")]
    uri: Uri,
    #[serde(deserialize_with = "deserialize_http_method")]
    method: Method,
    host: String,
    #[serde(deserialize_with = "deserialize_http_header_map")]
    headers: HeaderMap,
}

impl JsonLineRecord {
    /// Loads HTTP requests in JSON ammo format from specified path.
    pub fn from_fs<P, T>(path: P) -> Result<Vec<T>, Box<dyn Error>>
    where
        P: AsRef<Path>,
        T: TryFrom<JsonLineRecord, Error = Box<dyn Error>>,
    {
        log::debug!("loading HTTP requests from '{}' ...", path.as_ref().display());

        let rd = File::open(path)?;
        let rd = BufReader::new(rd);

        let mut requests = Vec::new();
        for line in rd.lines() {
            let line = line?;
            let record: JsonLineRecord = serde_json::from_str(&line)?;
            let request = record.try_into()?;

            requests.push(request);
        }

        Ok(requests)
    }

    /// Returns this [`JsonLineRecord`] as a raw HTTP request.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);

        buf.extend_from_slice(self.method.as_str().as_bytes());
        buf.push(b' ');
        // URI is relative, which is enforced during deserialization.
        buf.extend_from_slice(self.uri.path().as_bytes());

        if let Some(query) = self.uri.query() {
            buf.push(b'?');
            buf.extend_from_slice(query.as_bytes());
        }
        buf.extend_from_slice(b" HTTP/1.1\r\n");

        // Set "Host" header explicitly.
        buf.extend_from_slice(b"Host: ");
        buf.extend_from_slice(self.host.as_bytes());
        buf.extend_from_slice(b"\r\n");

        for (name, value) in &self.headers {
            if name == header::HOST {
                continue;
            }

            buf.extend_from_slice(name.as_str().as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        buf.extend_from_slice(b"\r\n");

        buf
    }
}

impl TryFrom<JsonLineRecord> for Request<Empty<Bytes>> {
    type Error = Box<dyn Error>;

    fn try_from(v: JsonLineRecord) -> Result<Self, Self::Error> {
        let mut request = Request::builder()
            .method(v.method)
            .uri(v.uri)
            .header(header::HOST, v.host);

        for (name, value) in v.headers.into_iter() {
            if let Some(name) = name {
                request = request.header(name, value);
            }
        }
        let request = request.body(Empty::new())?;

        Ok(request)
    }
}

impl TryFrom<JsonLineRecord> for Bytes {
    type Error = Box<dyn Error>;

    #[inline]
    fn try_from(v: JsonLineRecord) -> Result<Self, Self::Error> {
        Ok(v.to_vec().into())
    }
}

/// HTTP/3 request payload.
///
/// Unlike HTTP/1, the `h3` client takes the request head as `Request<()>` and
/// streams the (here empty) body separately, so the payload type carries no
/// body.
///
/// In HTTP/3 the `Host` header is replaced by the `:authority` pseudo-header
/// (RFC 9114 §4.3.1). The `h3` crate derives `:authority` exclusively from the
/// authority component of the request URI — it does **not** promote a `Host`
/// header into `:authority` automatically. Therefore the dedicated `host` field
/// is embedded into the URI as its authority so that `h3` emits a correct
/// `:authority` pseudo-header. Any `Host` entry in the record's `headers` map
/// is dropped: it would either duplicate or contradict the URI authority.
impl TryFrom<JsonLineRecord> for Request<()> {
    type Error = Box<dyn Error>;

    fn try_from(v: JsonLineRecord) -> Result<Self, Self::Error> {
        // Rebuild the URI with authority so that `h3` emits `:authority`.
        // The URI validator guarantees the record URI is relative (path only),
        // so the only authority component is the `host` field.
        let uri = {
            let mut parts = v.uri.into_parts();
            parts.scheme = Some(http::uri::Scheme::HTTPS);
            parts.authority = Some(v.host.parse()?);
            Uri::from_parts(parts)?
        };

        let mut request = Request::builder().method(v.method).uri(uri);

        for (name, value) in v.headers.into_iter() {
            if let Some(name) = name {
                if name == header::HOST {
                    continue;
                }
                request = request.header(name, value);
            }
        }
        let request = request.body(())?;

        Ok(request)
    }
}

fn deserialize_http_uri<'de, D>(de: D) -> Result<Uri, D::Error>
where
    D: Deserializer<'de>,
{
    let v: String = Deserialize::deserialize(de)?;
    match Uri::from_str(&v) {
        Ok(v) => {
            if v.authority().is_some() || v.scheme().is_some() {
                return Err(de::Error::invalid_value(
                    Unexpected::Str(&v.to_string()),
                    &"URI must be relative",
                ));
            }
            if v.path().is_empty() {
                return Err(de::Error::invalid_value(
                    Unexpected::Str(&v.to_string()),
                    &"URI must have path",
                ));
            }

            Ok(v)
        }
        Err(err) => {
            let err = format!("{}", err);
            Err(de::Error::invalid_value(Unexpected::Str(&v), &err.as_str()))
        }
    }
}

fn deserialize_http_method<'de, D>(de: D) -> Result<Method, D::Error>
where
    D: Deserializer<'de>,
{
    let v: String = Deserialize::deserialize(de)?;
    match Method::from_bytes(v.as_bytes()) {
        Ok(v) => Ok(v),
        Err(err) => {
            let err = format!("{}", err);
            Err(de::Error::invalid_value(Unexpected::Str(&v), &err.as_str()))
        }
    }
}

fn deserialize_http_header_map<'de, D>(de: D) -> Result<HeaderMap, D::Error>
where
    D: Deserializer<'de>,
{
    let v: HashMap<String, String> = Deserialize::deserialize(de)?;
    let mut headers = HeaderMap::new();

    for (name, value) in v {
        let name = match HeaderName::from_str(&name) {
            Ok(name) => name,
            Err(err) => {
                let err = format!("{}", err);
                return Err(de::Error::invalid_value(Unexpected::Str(&value), &err.as_str()));
            }
        };

        let value = match HeaderValue::from_bytes(value.as_bytes()) {
            Ok(v) => v,
            Err(err) => {
                let err = format!("{}", err);
                return Err(de::Error::invalid_value(Unexpected::Str(&value), &err.as_str()));
            }
        };

        headers.insert(name, value);
    }

    Ok(headers)
}

#[cfg(test)]
mod tests {
    use http::{header, uri::Scheme, Method, Request};

    use super::JsonLineRecord;

    fn parse(line: &str) -> JsonLineRecord {
        serde_json::from_str(line).expect("valid record")
    }

    #[test]
    fn http3_request_maps_head_and_host() {
        let record = parse(r#"{"uri":"/ping?x=1","method":"POST","host":"example.com","headers":{"X-Test":"1"}}"#);

        let req: Request<()> = record.try_into().expect("convertible");

        assert_eq!(req.method(), Method::POST);
        // The host is now in the URI authority, not in a Host header.
        assert_eq!(req.uri().authority().unwrap().as_str(), "example.com");
        assert_eq!(req.uri().scheme().unwrap(), &Scheme::HTTPS);
        assert_eq!(req.uri().path(), "/ping");
        assert_eq!(req.uri().query(), Some("x=1"));
        // No Host header — authority is carried by :authority pseudo-header.
        assert!(req.headers().get(header::HOST).is_none());
        assert_eq!(req.headers().get("x-test").unwrap(), "1");
    }

    #[test]
    fn http3_request_prefers_top_level_host_over_header() {
        // A "Host" inside headers must not shadow the dedicated `host` field.
        // The top-level host becomes the URI authority; the Host header entry
        // is dropped entirely.
        let record = parse(r#"{"uri":"/","method":"GET","host":"real.host","headers":{"Host":"ignored.host"}}"#);

        let req: Request<()> = record.try_into().expect("convertible");

        assert_eq!(req.uri().authority().unwrap().as_str(), "real.host");
        assert!(req.headers().get(header::HOST).is_none());
    }
}
