use super::api_error;
use evm_abstract_protocol::{API_PATH, ApiError, ApiErrorCode, MAX_REQUEST_BYTES};
use std::{
    io::{BufRead, BufReader, Read},
    net::TcpStream,
    time::{Duration, Instant},
};

pub(super) struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

pub(super) fn read(stream: &mut TcpStream) -> Result<Request, ApiError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut reader = BufReader::new(stream);
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= 8192 {
            return Err(api_error(
                ApiErrorCode::RequestTooLarge,
                "HTTP headers exceed 8192 bytes",
            ));
        }
        set_deadline(&reader, deadline)?;
        let available = reader.fill_buf().map_err(|error| {
            api_error(
                ApiErrorCode::InvalidRequest,
                format!("header read failed: {error}"),
            )
        })?;
        if available.is_empty() {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "incomplete HTTP headers",
            ));
        }
        // Consume exactly the header, leaving a pipelined body in the buffer.
        header.push(available[0]);
        reader.consume(1);
    }
    let header = std::str::from_utf8(&header)
        .map_err(|_| api_error(ApiErrorCode::InvalidRequest, "HTTP headers must be UTF-8"))?;
    let mut lines = header[..header.len() - 4].split("\r\n");
    let mut start = lines.next().unwrap_or_default().split(' ');
    let method = start.next().unwrap_or_default();
    let path = start.next().unwrap_or_default();
    let version = start.next().unwrap_or_default();
    if start.next().is_some()
        || method.is_empty()
        || !path.starts_with('/')
        || path.starts_with("//")
        || !matches!(version, "HTTP/1.1" | "HTTP/1.0")
        || path.chars().any(char::is_control)
    {
        return Err(api_error(
            ApiErrorCode::InvalidRequest,
            "invalid HTTP request line",
        ));
    }
    let mut length = None;
    let mut content_type = None;
    let mut host = None;
    let mut origin = None;
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "invalid HTTP header",
            ));
        };
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "invalid HTTP header name",
            ));
        }
        let value = value.trim();
        if key.eq_ignore_ascii_case("content-length") {
            if length.is_some()
                || value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(api_error(
                    ApiErrorCode::InvalidRequest,
                    "invalid or duplicate Content-Length",
                ));
            }
            length = Some(value.parse::<usize>().map_err(|_| {
                api_error(
                    ApiErrorCode::RequestTooLarge,
                    "Content-Length exceeds request bound",
                )
            })?);
        } else if key.eq_ignore_ascii_case("transfer-encoding") {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "Transfer-Encoding is unsupported; send Content-Length",
            ));
        } else if key.eq_ignore_ascii_case("content-type") {
            if content_type.replace(value).is_some() {
                return Err(api_error(
                    ApiErrorCode::InvalidRequest,
                    "duplicate Content-Type",
                ));
            }
        } else if key.eq_ignore_ascii_case("host") {
            if host.replace(value).is_some() {
                return Err(api_error(ApiErrorCode::InvalidRequest, "duplicate Host"));
            }
        } else if key.eq_ignore_ascii_case("origin") && origin.replace(value).is_some() {
            return Err(api_error(ApiErrorCode::InvalidRequest, "duplicate Origin"));
        }
    }
    if version == "HTTP/1.1" && host.is_none_or(str::is_empty) {
        return Err(api_error(
            ApiErrorCode::InvalidRequest,
            "HTTP/1.1 requires Host",
        ));
    }
    if let Some(origin) = origin
        && !host.is_some_and(|host| origin == format!("http://{host}"))
    {
        return Err(api_error(
            ApiErrorCode::InvalidRequest,
            "analysis host only accepts same-origin requests",
        ));
    }
    if method == "POST" && path == API_PATH {
        if length.is_none() {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "analysis JSON requires Content-Length",
            ));
        }
        if !content_type.is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        }) {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "analysis requires application/json",
            ));
        }
    }
    let length = length.unwrap_or(0);
    if length > MAX_REQUEST_BYTES {
        return Err(api_error(
            ApiErrorCode::RequestTooLarge,
            "JSON request exceeds 262144 bytes",
        ));
    }
    let mut body = vec![0; length];
    let mut offset = 0;
    while offset < length {
        set_deadline(&reader, deadline)?;
        let count = reader.read(&mut body[offset..]).map_err(|error| {
            api_error(
                ApiErrorCode::InvalidRequest,
                format!("body read failed: {error}"),
            )
        })?;
        if count == 0 {
            return Err(api_error(
                ApiErrorCode::InvalidRequest,
                "request body shorter than Content-Length",
            ));
        }
        offset += count;
    }
    Ok(Request {
        method: method.into(),
        path: path.into(),
        body,
    })
}

fn set_deadline(reader: &BufReader<&mut TcpStream>, deadline: Instant) -> Result<(), ApiError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            api_error(
                ApiErrorCode::InvalidRequest,
                "request read deadline exceeded",
            )
        })?;
    reader
        .get_ref()
        .set_read_timeout(Some(remaining))
        .map_err(|error| {
            api_error(
                ApiErrorCode::Internal,
                format!("socket timeout setup failed: {error}"),
            )
        })
}
