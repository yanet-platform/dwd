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

    pub fn to_bytes(&self) -> Bytes {
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

        buf.into()
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
        Ok(v.to_bytes())
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
