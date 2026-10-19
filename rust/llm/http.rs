use std::future::Future;

use reqwest::Client;
use url::Url;

use crate::Error;
use crate::text::Text;

use super::{Failure, MAX_RESPONSE_BYTES};

pub struct Request {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct Response<B> {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Option<B>,
}

impl<B> Response<B> {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub trait Body {
    fn read(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, Error>>;
}

pub trait Transport {
    type Body: Body;

    fn send(
        &mut self,
        request: &Request,
    ) -> impl Future<Output = Result<Response<Self::Body>, Failure>>;
}

pub fn validate_url(input: &Text) -> Result<Url, Error> {
    let url = Url::parse(&String::from_utf16_lossy(&input.0))
        .map_err(|_| Error::type_error("Invalid URL").coded("ERR_INVALID_URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::type_error(
            "LLM transport requires an HTTP(S) URL without credentials",
        ));
    }
    Ok(url)
}

pub struct HttpTransport {
    client: Client,
}

impl HttpTransport {
    pub fn new() -> Result<Self, Failure> {
        Self::with_root_certificate(None)
    }

    pub fn with_root_certificate(certificate: Option<&[u8]>) -> Result<Self, Failure> {
        let mut builder = Client::builder()
            .http1_only()
            .http1_max_headers(2000)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .pool_max_idle_per_host(0)
            .no_proxy();
        if let Some(certificate) = certificate {
            let certificate =
                reqwest::Certificate::from_pem(certificate).map_err(|_| Failure::network())?;
            builder = builder.tls_certs_merge([certificate]);
        }
        Ok(Self {
            client: builder.build().map_err(|_| Failure::network())?,
        })
    }
}

pub struct NetworkBody {
    response: Option<reqwest::Response>,
    bytes: usize,
    failure: Option<Error>,
}

impl NetworkBody {
    fn fail(&mut self, error: Error) -> Error {
        self.response.take();
        self.failure = Some(error.clone());
        error
    }
}

impl Body for NetworkBody {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let Some(response) = &mut self.response else {
            return self.failure.clone().map_or(Ok(None), Err);
        };
        match response.chunk().await {
            Ok(Some(bytes)) => {
                let Some(total) = self
                    .bytes
                    .checked_add(bytes.len())
                    .filter(|bytes| *bytes <= MAX_RESPONSE_BYTES)
                else {
                    return Err(self.fail(
                        Error::new("Error", "LLM response body exceeded 32 MiB")
                            .coded("ROSTER_LLM_BODY_LIMIT"),
                    ));
                };
                self.bytes = total;
                Ok(Some(bytes.to_vec()))
            }
            Ok(None) => {
                self.response.take();
                Ok(None)
            }
            Err(_) => Err(self.fail(Error::new("Error", "LLM response body read failed"))),
        }
    }
}

impl Transport for HttpTransport {
    type Body = NetworkBody;

    async fn send(&mut self, request: &Request) -> Result<Response<NetworkBody>, Failure> {
        let url = validate_url(&Text::from(request.url.as_str()))?;
        if request.body.len() > MAX_RESPONSE_BYTES {
            return Err(Error::type_error("The LLM request exceeded the supported size.").into());
        }
        let header_bytes = request
            .headers
            .iter()
            .try_fold(0_usize, |total, (name, value)| {
                total
                    .checked_add(name.len())?
                    .checked_add(value.len())?
                    .checked_add(4)
            });
        if header_bytes.is_none_or(|bytes| bytes > 16 * 1024) {
            return Err(
                Error::type_error("The LLM request headers exceeded the supported size.").into(),
            );
        }
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| Failure::network())?;
        let mut outgoing = self
            .client
            .request(method, url)
            .header("Accept-Encoding", "identity")
            .header("Connection", "close");
        for (name, value) in &request.headers {
            outgoing = outgoing.header(name, value);
        }
        let response = outgoing
            .body(request.body.clone())
            .send()
            .await
            .map_err(|_| Failure::network())?;
        let status = response.status().as_u16();
        let header_bytes = response
            .headers()
            .iter()
            .try_fold(0_usize, |total, (name, value)| {
                total
                    .checked_add(name.as_str().len())?
                    .checked_add(value.as_bytes().len())?
                    .checked_add(4)
            });
        if status == 101 || header_bytes.is_none_or(|bytes| bytes > 16 * 1024) {
            return Err(Failure::network());
        }
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect();
        let body = if matches!(status, 204 | 205 | 304) {
            None
        } else {
            Some(NetworkBody {
                response: Some(response),
                bytes: 0,
                failure: None,
            })
        };
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}
