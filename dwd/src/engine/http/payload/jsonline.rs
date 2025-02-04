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
pub struct JsonLinePayload {
    #[serde(deserialize_with = "deserialize_http_uri")]
    uri: Uri,
    #[serde(deserialize_with = "deserialize_http_method")]
    method: Method,
    host: String,
    #[serde(deserialize_with = "deserialize_http_header_map")]
    headers: HeaderMap,
}

impl JsonLinePayload {
    /// Loads HTTP requests in JSON ammo format from specified path.
    pub fn from_fs<P>(path: P) -> Result<Vec<Request<Empty<Bytes>>>, Box<dyn Error>>
    where
        P: AsRef<Path>,
    {
        log::debug!("loading HTTP requests from '{}' ...", path.as_ref().display());

        let rd = File::open(path)?;
        let rd = BufReader::new(rd);

        let mut requests = Vec::new();
        for line in rd.lines() {
            let line = line?;
            let payload: JsonLinePayload = serde_json::from_str(&line)?;
            let mut request = Request::builder()
                .method(payload.method)
                .uri(payload.uri)
                .header(header::HOST, payload.host);

            for (name, value) in payload.headers.into_iter() {
                if let Some(name) = name {
                    request = request.header(name, value);
                }
            }
            let request = request.body(Empty::new())?;
            log::debug!("loaded HTTP request: {:?}", request);

            requests.push(request);
        }

        Ok(requests)
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
