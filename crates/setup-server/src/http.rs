//! The accepted HTTP/1.1 subset (docs/design/phone-setup.md, section 6.4).
//!
//! One request per connection, `HTTP/1.1` only, request line and headers at
//! most 4 KiB with at most 24 headers, CRLF line ends only, visible ASCII
//! only, no folded headers, no `Transfer-Encoding`, `Expect`, `Upgrade` or
//! `HTTP2-Settings`, no pipelining, a body only on `POST` with exactly one
//! `Content-Length` of at most 1 KiB. Everything else is an error: this
//! parser rejects rather than interprets.

use std::fmt::{self, Write as _};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Request line and headers, including the final empty line.
pub(crate) const MAX_HEAD: usize = 4096;
/// Header lines after the request line.
pub(crate) const MAX_HEADERS: usize = 24;
/// Request body (`POST` only).
pub(crate) const MAX_BODY: usize = 1024;
/// Request target (a path; the longest real one is `/s/` plus 43 characters).
pub(crate) const MAX_TARGET: usize = 128;
const MAX_METHOD: usize = 16;
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
    Head,
    /// Any other well-formed method token (`OPTIONS`, `PUT`, ...): 405 or 404.
    Other,
}

impl Method {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Head => "HEAD",
            Method::Other => "OTHER",
        }
    }
}

/// A parsed request. Only the headers the server uses are kept; the rest are
/// validated and dropped.
pub(crate) struct Request {
    pub(crate) method: Method,
    /// The path (absolute path form, restricted characters, no query).
    pub(crate) target: String,
    pub(crate) host: String,
    pub(crate) cookie: Option<String>,
    pub(crate) content_type: Option<String>,
    pub(crate) origin: Option<String>,
    pub(crate) sec_fetch_site: Option<String>,
    pub(crate) csrf: Option<String>,
    pub(crate) body: Vec<u8>,
}

// The target (the token on the first request), the cookie and the CSRF header
// are secrets: Debug prints neither.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("target_len", &self.target.len())
            .field("cookie", &self.cookie.is_some())
            .field("csrf", &self.csrf.is_some())
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// Why a request was not accepted. Each one is a parse error in the sense of
/// the design (section 4.2), except [`ReadError::Empty`] in [`read_request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// 431: request line and headers over 4 KiB, or over 24 headers.
    HeadTooLarge,
    /// 413: `Content-Length` over 1 KiB.
    BodyTooLarge,
    /// 411: a `POST` without `Content-Length`.
    LengthRequired,
    /// 505: an HTTP version other than 1.1.
    Version,
    /// 408: the request was not complete within the time limit.
    Timeout,
    /// 400: anything else outside the subset (the word is for the log).
    Bad(&'static str),
}

impl ParseError {
    pub(crate) fn status(self) -> u16 {
        match self {
            ParseError::HeadTooLarge => 431,
            ParseError::BodyTooLarge => 413,
            ParseError::LengthRequired => 411,
            ParseError::Version => 505,
            ParseError::Timeout => 408,
            ParseError::Bad(_) => 400,
        }
    }

    /// A fixed word for the log (never request bytes).
    pub(crate) fn kind(self) -> &'static str {
        match self {
            ParseError::HeadTooLarge => "head-too-large",
            ParseError::BodyTooLarge => "body-too-large",
            ParseError::LengthRequired => "length-required",
            ParseError::Version => "version",
            ParseError::Timeout => "timeout",
            ParseError::Bad(w) => w,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Parsed {
    Incomplete,
    Done(Request),
}

/// Parses `buf`, everything received so far on the connection.
pub(crate) fn parse(buf: &[u8]) -> Result<Parsed, ParseError> {
    let window = &buf[..buf.len().min(MAX_HEAD)];
    let Some(end) = find_head_end(window) else {
        // Fail fast on bytes that can never become a valid head.
        check_head_bytes(window, false)?;
        if buf.len() >= MAX_HEAD {
            return Err(ParseError::HeadTooLarge);
        }
        return Ok(Parsed::Incomplete);
    };
    // `end` is the index after "\r\n\r\n"; the head is everything before the
    // final CRLF pair.
    let (mut req, len) = parse_head(&buf[..end - 4])?;
    let total = end + len;
    if buf.len() < total {
        return Ok(Parsed::Incomplete);
    }
    if buf.len() > total {
        return Err(ParseError::Bad("pipelined"));
    }
    req.body = buf[end..].to_vec();
    Ok(Parsed::Done(req))
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Only visible ASCII, space, tab, and CR only as part of CRLF. `complete`:
/// `buf` is a whole head (a CR at its end is then invalid too).
fn check_head_bytes(buf: &[u8], complete: bool) -> Result<(), ParseError> {
    for (i, &b) in buf.iter().enumerate() {
        match b {
            b'\r' => match buf.get(i + 1) {
                Some(b'\n') => {}
                None if !complete => {}
                _ => return Err(ParseError::Bad("bare-cr")),
            },
            b'\n' => {
                if i == 0 || buf[i - 1] != b'\r' {
                    return Err(ParseError::Bad("bare-lf"));
                }
            }
            b'\t' | 0x20..=0x7e => {}
            _ => return Err(ParseError::Bad("byte")),
        }
    }
    Ok(())
}

// Header slots that must not repeat.
const H_HOST: u8 = 1;
const H_LENGTH: u8 = 1 << 1;
const H_TYPE: u8 = 1 << 2;
const H_COOKIE: u8 = 1 << 3;
const H_ORIGIN: u8 = 1 << 4;
const H_FETCH_SITE: u8 = 1 << 5;
const H_CSRF: u8 = 1 << 6;

/// Parses the head (without the final CRLF CRLF); returns the request and
/// its body length.
fn parse_head(head: &[u8]) -> Result<(Request, usize), ParseError> {
    check_head_bytes(head, true)?;
    let text = std::str::from_utf8(head).map_err(|_| ParseError::Bad("byte"))?;
    let mut lines = text.split("\r\n");
    let (method, target) = request_line(lines.next().unwrap_or(""))?;
    let mut req = Request {
        method,
        target,
        host: String::new(),
        cookie: None,
        content_type: None,
        origin: None,
        sec_fetch_site: None,
        csrf: None,
        body: Vec::new(),
    };
    let mut seen = 0u8;
    let mut length: Option<usize> = None;
    let mut count = 0usize;
    for line in lines {
        count += 1;
        if count > MAX_HEADERS {
            return Err(ParseError::HeadTooLarge);
        }
        if line.starts_with([' ', '\t']) {
            return Err(ParseError::Bad("folded"));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ParseError::Bad("header"));
        };
        if name.is_empty() || name.len() > MAX_NAME || !name.bytes().all(is_tchar) {
            return Err(ParseError::Bad("header-name"));
        }
        let value = value.trim_matches([' ', '\t']);
        let name = name.to_ascii_lowercase();
        let slot = match name.as_str() {
            "host" => H_HOST,
            "content-length" => H_LENGTH,
            "content-type" => H_TYPE,
            "cookie" => H_COOKIE,
            "origin" => H_ORIGIN,
            "sec-fetch-site" => H_FETCH_SITE,
            "x-setup-csrf" => H_CSRF,
            "transfer-encoding" | "expect" | "upgrade" | "http2-settings" => {
                return Err(ParseError::Bad("forbidden-header"));
            }
            _ => 0,
        };
        if slot != 0 {
            if seen & slot != 0 {
                return Err(ParseError::Bad("duplicate-header"));
            }
            seen |= slot;
        }
        let owned = value.to_owned();
        match slot {
            H_HOST => req.host = owned,
            H_LENGTH => length = Some(content_length(value)?),
            H_TYPE => req.content_type = Some(owned),
            H_COOKIE => req.cookie = Some(owned),
            H_ORIGIN => req.origin = Some(owned),
            H_FETCH_SITE => req.sec_fetch_site = Some(owned),
            H_CSRF => req.csrf = Some(owned),
            _ => {}
        }
    }
    if req.host.is_empty() {
        return Err(ParseError::Bad("host"));
    }
    let len = match (req.method, length) {
        (Method::Post, None) => return Err(ParseError::LengthRequired),
        (Method::Post, Some(n)) => n,
        (_, None | Some(0)) => 0,
        (_, Some(_)) => return Err(ParseError::Bad("body-not-allowed")),
    };
    Ok((req, len))
}

fn request_line(line: &str) -> Result<(Method, String), ParseError> {
    let mut parts = line.split(' ');
    let (Some(m), Some(t), Some(v), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::Bad("request-line"));
    };
    if m.is_empty() || m.len() > MAX_METHOD || !m.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(ParseError::Bad("method"));
    }
    if v != "HTTP/1.1" {
        return Err(if v.starts_with("HTTP/") {
            ParseError::Version
        } else {
            ParseError::Bad("version")
        });
    }
    if !t.starts_with('/') || t.len() > MAX_TARGET || !t.bytes().all(is_path_byte) {
        return Err(ParseError::Bad("target"));
    }
    let method = match m {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "HEAD" => Method::Head,
        _ => Method::Other,
    };
    Ok((method, t.to_owned()))
}

/// RFC 9110 token characters.
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// Unreserved characters and `/`: no query, no percent-encoding, no `;`.
fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~/".contains(&b)
}

fn content_length(v: &str) -> Result<usize, ParseError> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::Bad("length"));
    }
    if v.len() > 1 && v.starts_with('0') {
        return Err(ParseError::Bad("length"));
    }
    if v.len() > 5 {
        return Err(ParseError::BodyTooLarge);
    }
    let n: usize = v.parse().map_err(|_| ParseError::Bad("length"))?;
    if n > MAX_BODY {
        return Err(ParseError::BodyTooLarge);
    }
    Ok(n)
}

/// A connection the request is read from (a socket, or a fake in tests).
pub(crate) trait Conn: Read {
    fn set_timeout(&mut self, d: Duration) -> io::Result<()>;
}

impl Conn for TcpStream {
    fn set_timeout(&mut self, d: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(d))
    }
}

/// What reading a connection gave.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// Closed, failed or timed out before a single byte arrived (a browser's
    /// speculative connection): no response, not a parse error.
    Empty,
    Parse(ParseError),
}

/// Reads one request, at most `MAX_HEAD + MAX_BODY` bytes, until `deadline`.
/// `abort` is asked about every 100 ms (a stop from the TV must not wait for
/// a slow client); when it says yes the read ends as [`ReadError::Empty`].
pub(crate) fn read_request<C: Conn>(
    c: &mut C,
    deadline: Instant,
    mut abort: impl FnMut() -> bool,
) -> Result<Request, ReadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let fail = |buf: &[u8], e: ParseError| {
        if buf.is_empty() {
            ReadError::Empty
        } else {
            ReadError::Parse(e)
        }
    };
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(fail(&buf, ParseError::Timeout));
        }
        if abort() {
            return Err(ReadError::Empty);
        }
        if c.set_timeout(left.min(Duration::from_millis(100))).is_err() {
            return Err(fail(&buf, ParseError::Bad("io")));
        }
        // parse() rejects anything longer, so this never reaches zero.
        let want = (MAX_HEAD + MAX_BODY + 1)
            .saturating_sub(buf.len())
            .clamp(1, chunk.len());
        match c.read(&mut chunk[..want]) {
            Ok(0) => return Err(fail(&buf, ParseError::Bad("truncated"))),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n.min(want)]);
                match parse(&buf) {
                    Ok(Parsed::Done(r)) => return Ok(r),
                    Ok(Parsed::Incomplete) => {
                        if buf.len() > MAX_HEAD + MAX_BODY {
                            return Err(ReadError::Parse(ParseError::Bad("too-long")));
                        }
                    }
                    Err(e) => return Err(ReadError::Parse(e)),
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err(fail(&buf, ParseError::Bad("io"))),
        }
    }
}

/// A connection a response is written to (a socket, or a fake in tests).
pub(crate) trait Sink: Write {
    fn set_write_limit(&mut self, d: Duration) -> io::Result<()>;
}

impl Sink for TcpStream {
    fn set_write_limit(&mut self, d: Duration) -> io::Result<()> {
        self.set_write_timeout(Some(d))
    }
}

/// Writes the whole response before `deadline` or gives up (section 6.4: 3 s
/// to write the response). The socket's write timeout alone limits each
/// `write` call, not the response: a client that reads a few bytes now and
/// then would hold the single-threaded loop (and the stop, the heartbeat and
/// the lifetime checks) for many such calls. Security review SR-01.
pub(crate) fn write_response<S: Sink>(
    s: &mut S,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    let mut off = 0;
    while off < bytes.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        s.set_write_limit(left)?;
        match s.write(&bytes[off..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => off += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    s.flush()
}

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// A response; the common headers of section 8 are added when serialized.
#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) status: u16,
    content_type: Option<&'static str>,
    html: bool,
    headers: Vec<(&'static str, String)>,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub(crate) fn empty(status: u16) -> Self {
        Response {
            status,
            content_type: None,
            html: false,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub(crate) fn json(status: u16, value: &serde_json::Value) -> Self {
        let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
        Response {
            status,
            content_type: Some("application/json; charset=utf-8"),
            html: false,
            headers: Vec::new(),
            body,
        }
    }

    /// `{"error": code, "message": sentence}`.
    pub(crate) fn error(status: u16, code: &str, message: &str) -> Self {
        Self::json(
            status,
            &serde_json::json!({"error": code, "message": message}),
        )
    }

    pub(crate) fn html(status: u16, body: &[u8]) -> Self {
        Response {
            status,
            content_type: Some("text/html; charset=utf-8"),
            html: true,
            headers: Vec::new(),
            body: body.to_vec(),
        }
    }

    pub(crate) fn asset(content_type: &'static str, body: &[u8]) -> Self {
        Response {
            status: 200,
            content_type: Some(content_type),
            html: false,
            headers: Vec::new(),
            body: body.to_vec(),
        }
    }

    /// Adds a header; a value with a control character is dropped (the
    /// server builds every value itself, so this never happens).
    #[must_use]
    pub(crate) fn header(mut self, name: &'static str, value: String) -> Self {
        if value.bytes().all(|b| (0x20..0x7f).contains(&b)) {
            self.headers.push((name, value));
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The response to a `HEAD` request: the same head, no body.
    #[must_use]
    pub(crate) fn without_body(mut self) -> Self {
        self.body.clear();
        self
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        if let Some(ct) = self.content_type {
            out.push_str("Content-Type: ");
            out.push_str(ct);
            out.push_str("\r\n");
        }
        let _ = write!(out, "Content-Length: {}\r\n", self.body.len());
        out.push_str("Connection: close\r\n");
        out.push_str("Cache-Control: no-store\r\n");
        out.push_str("X-Content-Type-Options: nosniff\r\n");
        out.push_str("Referrer-Policy: no-referrer\r\n");
        if self.html {
            out.push_str("Content-Security-Policy: ");
            out.push_str(CSP);
            out.push_str("\r\n");
            out.push_str("X-Frame-Options: DENY\r\n");
        }
        for (n, v) in &self.headers {
            out.push_str(n);
            out.push_str(": ");
            out.push_str(v);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        302 => "Found",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Internal Server Error",
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::too_many_lines,
        clippy::assert_is_empty,
        clippy::format_push_string,
        clippy::cast_possible_truncation
    )]

    use super::*;

    const HOST: &str = "Host: 192.168.1.50:49731\r\n";

    fn done(raw: &[u8]) -> Request {
        match parse(raw) {
            Ok(Parsed::Done(r)) => r,
            other => panic!("expected a request, got {other:?}"),
        }
    }

    fn err(raw: &[u8]) -> ParseError {
        match parse(raw) {
            Err(e) => e,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    fn get(path: &str) -> Vec<u8> {
        format!("GET {path} HTTP/1.1\r\n{HOST}\r\n").into_bytes()
    }

    fn post(body: &str, extra: &str) -> Vec<u8> {
        format!(
            "POST /api/actions HTTP/1.1\r\n{HOST}Content-Type: application/json\r\nContent-Length: {}\r\n{extra}\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn a_plain_get_parses() {
        let r = done(
            b"GET /api/state HTTP/1.1\r\nHost: 192.168.1.50:49731\r\nCookie: sat_session=abc\r\nAccept: */*\r\n\r\n",
        );
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.target, "/api/state");
        assert_eq!(r.host, "192.168.1.50:49731");
        assert_eq!(r.cookie.as_deref(), Some("sat_session=abc"));
        assert!(r.body.is_empty());
    }

    #[test]
    fn header_names_are_case_insensitive_and_values_trimmed() {
        let r = done(b"GET / HTTP/1.1\r\nhOsT:   1.2.3.4:5 \t\r\nX-SETUP-CSRF: ab\r\n\r\n");
        assert_eq!(r.host, "1.2.3.4:5");
        assert_eq!(r.csrf.as_deref(), Some("ab"));
    }

    #[test]
    fn a_post_with_body_parses() {
        let r = done(&post(
            r#"{"action":"mic","value":"off"}"#,
            "X-Setup-CSRF: 00\r\n",
        ));
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.body, br#"{"action":"mic","value":"off"}"#);
        assert_eq!(r.content_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn a_post_with_an_empty_body_parses() {
        let r = done(&post("", ""));
        assert!(r.body.is_empty());
    }

    #[test]
    fn truncated_input_is_incomplete() {
        let full = post(r#"{"action":"pair"}"#, "");
        for cut in 0..full.len() {
            match parse(&full[..cut]) {
                Ok(Parsed::Incomplete) => {}
                other => panic!("cut {cut}: {other:?}"),
            }
        }
        assert!(matches!(parse(&full), Ok(Parsed::Done(_))));
    }

    #[test]
    fn pipelined_requests_are_refused() {
        let mut two = get("/");
        two.extend_from_slice(&get("/api/state"));
        assert_eq!(err(&two), ParseError::Bad("pipelined"));
        let mut extra = post("{}", "");
        extra.push(b'x');
        assert_eq!(err(&extra), ParseError::Bad("pipelined"));
    }

    #[test]
    fn oversize_heads_are_431() {
        let big = format!(
            "GET / HTTP/1.1\r\n{HOST}X-Pad: {}\r\n\r\n",
            "a".repeat(MAX_HEAD)
        );
        assert_eq!(err(big.as_bytes()), ParseError::HeadTooLarge);
        // Even before the end of the head arrived.
        assert_eq!(
            err(&big.as_bytes()[..MAX_HEAD + 10]),
            ParseError::HeadTooLarge
        );
        // Exactly at the limit is accepted.
        let base = format!("GET / HTTP/1.1\r\n{HOST}X-Pad: \r\n\r\n").len();
        let fit = format!(
            "GET / HTTP/1.1\r\n{HOST}X-Pad: {}\r\n\r\n",
            "a".repeat(MAX_HEAD - base)
        );
        assert_eq!(fit.len(), MAX_HEAD);
        assert!(matches!(parse(fit.as_bytes()), Ok(Parsed::Done(_))));
        let over = format!(
            "GET / HTTP/1.1\r\n{HOST}X-Pad: {}\r\n\r\n",
            "a".repeat(MAX_HEAD - base + 1)
        );
        assert_eq!(err(over.as_bytes()), ParseError::HeadTooLarge);
    }

    #[test]
    fn more_than_24_headers_are_431() {
        let mut ok = format!("GET / HTTP/1.1\r\n{HOST}");
        for i in 1..MAX_HEADERS {
            ok.push_str(&format!("X-{i}: v\r\n"));
        }
        let fits = format!("{ok}\r\n");
        assert!(matches!(parse(fits.as_bytes()), Ok(Parsed::Done(_))));
        let over = format!("{ok}X-last: v\r\n\r\n");
        assert_eq!(err(over.as_bytes()), ParseError::HeadTooLarge);
    }

    #[test]
    fn bodies_over_1_kib_are_413() {
        let body = "a".repeat(MAX_BODY);
        assert!(matches!(parse(&post(&body, "")), Ok(Parsed::Done(_))));
        let raw = format!(
            "POST /api/actions HTTP/1.1\r\n{HOST}Content-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert_eq!(err(raw.as_bytes()), ParseError::BodyTooLarge);
        let huge = format!("POST / HTTP/1.1\r\n{HOST}Content-Length: 99999999999999999999\r\n\r\n");
        assert_eq!(err(huge.as_bytes()), ParseError::BodyTooLarge);
    }

    #[test]
    fn content_length_rules() {
        let cases: &[(&str, ParseError)] = &[
            (
                "POST / HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::LengthRequired,
            ),
            (
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
                ParseError::Bad("duplicate-header"),
            ),
            (
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: +1\r\n\r\nx",
                ParseError::Bad("length"),
            ),
            (
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: -1\r\n\r\n",
                ParseError::Bad("length"),
            ),
            (
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 1,1\r\n\r\nx",
                ParseError::Bad("length"),
            ),
            (
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 01\r\n\r\nx",
                ParseError::Bad("length"),
            ),
            (
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: \r\n\r\n",
                ParseError::Bad("length"),
            ),
            (
                "POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 0x1\r\n\r\nx",
                ParseError::Bad("length"),
            ),
            (
                "GET / HTTP/1.1\r\nHost: h\r\nContent-Length: 1\r\n\r\nx",
                ParseError::Bad("body-not-allowed"),
            ),
        ];
        for (raw, want) in cases {
            assert_eq!(err(raw.as_bytes()), *want, "{raw:?}");
        }
        // `Content-Length: 0` on GET is fine.
        assert!(matches!(
            parse(b"GET / HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\n\r\n"),
            Ok(Parsed::Done(_))
        ));
    }

    #[test]
    fn chunked_and_other_framing_headers_are_refused() {
        for h in [
            "Transfer-Encoding: chunked",
            "transfer-encoding: identity",
            "Expect: 100-continue",
            "Upgrade: websocket",
            "HTTP2-Settings: AAA",
        ] {
            let raw = format!("POST / HTTP/1.1\r\nHost: h\r\n{h}\r\nContent-Length: 0\r\n\r\n");
            assert_eq!(
                err(raw.as_bytes()),
                ParseError::Bad("forbidden-header"),
                "{h}"
            );
        }
    }

    #[test]
    fn smuggling_shapes_are_refused() {
        let cases: &[&[u8]] = &[
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length : 0\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: h\r\n Content-Length: 0\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\n\tx\r\n\r\n",
            b"POST / HTTP/1.1\nHost: h\nContent-Length: 0\n\n",
            b"POST / HTTP/1.1\r\nHost: h\rContent-Length: 0\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nHost: evil\r\n\r\n",
            b"GET / HTTP/1.1\r\nHost: h\r\nX: a\x00b\r\n\r\n",
        ];
        for raw in cases {
            assert!(
                matches!(parse(raw), Err(ParseError::Bad(_))),
                "{:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn bad_request_lines() {
        let cases: &[(&[u8], ParseError)] = &[
            (b"GET / HTTP/1.0\r\nHost: h\r\n\r\n", ParseError::Version),
            (b"GET / HTTP/2\r\nHost: h\r\n\r\n", ParseError::Version),
            (
                b"GET / http/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("version"),
            ),
            (b"GET /\r\nHost: h\r\n\r\n", ParseError::Bad("request-line")),
            (
                b"GET  / HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("request-line"),
            ),
            (
                b"GET / HTTP/1.1 x\r\nHost: h\r\n\r\n",
                ParseError::Bad("request-line"),
            ),
            (
                b"get / HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("method"),
            ),
            (
                b"G3T / HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("method"),
            ),
            (
                b"VERYLONGMETHODNAME / HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("method"),
            ),
            (
                b"GET http://h/ HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("target"),
            ),
            (
                b"GET * HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("target"),
            ),
            (
                b"GET /a?b=c HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("target"),
            ),
            (
                b"GET /a%2e HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("target"),
            ),
            (
                b"GET /a#b HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("target"),
            ),
            (
                b"\r\nGET / HTTP/1.1\r\nHost: h\r\n\r\n",
                ParseError::Bad("request-line"),
            ),
            (b"\r\n\r\n", ParseError::Bad("request-line")),
            (b"GET / HTTP/1.1\r\n\r\n", ParseError::Bad("host")),
            (b"GET / HTTP/1.1\r\nHost:\r\n\r\n", ParseError::Bad("host")),
            (
                b"GET / HTTP/1.1\r\nNoColon\r\n\r\n",
                ParseError::Bad("header"),
            ),
            (
                b"GET / HTTP/1.1\r\n: v\r\n\r\n",
                ParseError::Bad("header-name"),
            ),
            (
                b"GET / HTTP/1.1\r\nBad(name): v\r\n\r\n",
                ParseError::Bad("header-name"),
            ),
        ];
        for (raw, want) in cases {
            assert_eq!(err(raw), *want, "{:?}", String::from_utf8_lossy(raw));
        }
        let long = format!(
            "GET /{} HTTP/1.1\r\nHost: h\r\n\r\n",
            "a".repeat(MAX_TARGET)
        );
        assert_eq!(err(long.as_bytes()), ParseError::Bad("target"));
    }

    #[test]
    fn other_methods_parse_for_405() {
        for (m, want) in [
            ("OPTIONS", Method::Other),
            ("PUT", Method::Other),
            ("HEAD", Method::Head),
        ] {
            let raw = format!("{m} / HTTP/1.1\r\nHost: h\r\n\r\n");
            assert_eq!(done(raw.as_bytes()).method, want);
        }
    }

    #[test]
    fn non_ascii_and_control_bytes_fail_early() {
        // Before the head is complete: no need to wait for the timeout.
        assert_eq!(err(b"GET /\xff"), ParseError::Bad("byte"));
        assert_eq!(err(b"GET /\x00"), ParseError::Bad("byte"));
        assert_eq!(err(b"GET / HTTP/1.1\n"), ParseError::Bad("bare-lf"));
        assert_eq!(err(b"GET / HTTP/1.1\rX"), ParseError::Bad("bare-cr"));
        assert_eq!(err(b"\x16\x03\x01\x02\x00"), ParseError::Bad("byte"));
        // A UTF-8 header value is refused.
        assert_eq!(
            err("GET / HTTP/1.1\r\nHost: h\r\nX: \u{e9}\r\n\r\n".as_bytes()),
            ParseError::Bad("byte")
        );
        // A CR at the very end of a partial buffer is fine (LF may follow).
        assert!(matches!(parse(b"GET / HTTP/1.1\r"), Ok(Parsed::Incomplete)));
    }

    #[test]
    fn duplicate_security_headers_are_refused() {
        for h in [
            "Cookie",
            "Origin",
            "Sec-Fetch-Site",
            "X-Setup-CSRF",
            "Content-Type",
        ] {
            let raw = format!("GET / HTTP/1.1\r\nHost: h\r\n{h}: a\r\n{h}: b\r\n\r\n");
            assert_eq!(
                err(raw.as_bytes()),
                ParseError::Bad("duplicate-header"),
                "{h}"
            );
        }
        // Other headers may repeat.
        assert!(matches!(
            parse(b"GET / HTTP/1.1\r\nHost: h\r\nAccept: a\r\nAccept: b\r\n\r\n"),
            Ok(Parsed::Done(_))
        ));
    }

    /// A deterministic pseudo-random corpus: no input may panic, and every
    /// result is one of the defined outcomes.
    #[test]
    fn random_inputs_never_panic() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let alphabet: &[u8] = b"GETPOST /HTTP1.\r\n:aAzZ09-_~\t\x00\xff%?#Content-Length: 12Host";
        let seeds: Vec<Vec<u8>> = vec![get("/"), post(r#"{"action":"mic","value":"on"}"#, "")];
        for i in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let mut input = seeds[i % seeds.len()].clone();
            let edits = (x % 6) as usize;
            for k in 0..edits {
                let r = x.rotate_left(k as u32 * 11);
                if input.is_empty() {
                    break;
                }
                let pos = (r as usize) % input.len();
                match r % 3 {
                    0 => input[pos] = alphabet[(r >> 8) as usize % alphabet.len()],
                    1 => {
                        input.remove(pos);
                    }
                    _ => input.insert(pos, alphabet[(r >> 16) as usize % alphabet.len()]),
                }
            }
            let _ = parse(&input);
        }
    }

    struct Fake {
        chunks: Vec<Vec<u8>>,
        delay: Duration,
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.chunks.is_empty() {
                std::thread::sleep(self.delay);
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let c = self.chunks.remove(0);
            let n = c.len().min(buf.len());
            buf[..n].copy_from_slice(&c[..n]);
            if n < c.len() {
                self.chunks.insert(0, c[n..].to_vec());
            }
            Ok(n)
        }
    }

    impl Conn for Fake {
        fn set_timeout(&mut self, _: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[test]
    fn read_request_assembles_byte_by_byte() {
        let raw = post(r#"{"action":"pair"}"#, "");
        let mut f = Fake {
            chunks: raw.iter().map(|b| vec![*b]).collect(),
            delay: Duration::from_millis(1),
        };
        let r = read_request(&mut f, deadline(2000), || false).expect("request");
        assert_eq!(r.body, br#"{"action":"pair"}"#);
    }

    #[test]
    fn slow_partial_requests_time_out() {
        let mut f = Fake {
            chunks: vec![b"GET / HTTP/1.1\r\nHost".to_vec()],
            delay: Duration::from_millis(10),
        };
        assert!(matches!(
            read_request(&mut f, deadline(100), || false),
            Err(ReadError::Parse(ParseError::Timeout))
        ));
        // A slow body too.
        let mut f = Fake {
            chunks: vec![b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\nab".to_vec()],
            delay: Duration::from_millis(10),
        };
        assert!(matches!(
            read_request(&mut f, deadline(100), || false),
            Err(ReadError::Parse(ParseError::Timeout))
        ));
    }

    #[test]
    fn a_silent_connection_is_empty_not_a_parse_error() {
        let mut f = Fake {
            chunks: vec![],
            delay: Duration::from_millis(10),
        };
        assert!(matches!(
            read_request(&mut f, deadline(50), || false),
            Err(ReadError::Empty)
        ));
        let mut f = Fake {
            chunks: vec![vec![]],
            delay: Duration::from_millis(1),
        };
        assert!(matches!(
            read_request(&mut f, deadline(50), || false),
            Err(ReadError::Empty)
        ));
    }

    #[test]
    fn a_stop_aborts_a_slow_read() {
        let mut f = Fake {
            chunks: vec![b"GET / HT".to_vec()],
            delay: Duration::from_millis(5),
        };
        let mut n = 0;
        let start = Instant::now();
        let r = read_request(&mut f, deadline(3000), || {
            n += 1;
            n > 3
        });
        assert!(matches!(r, Err(ReadError::Empty)));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_closed_partial_request_is_truncated() {
        let mut f = Fake {
            chunks: vec![b"GET / HT".to_vec(), vec![]],
            delay: Duration::from_millis(1),
        };
        assert!(matches!(
            read_request(&mut f, deadline(500), || false),
            Err(ReadError::Parse(ParseError::Bad("truncated")))
        ));
    }

    #[test]
    fn read_request_refuses_an_endless_head_without_waiting() {
        let mut f = Fake {
            chunks: vec![vec![b'A'; 10_000]],
            delay: Duration::from_millis(1),
        };
        assert!(matches!(
            read_request(&mut f, deadline(2000), || false),
            Err(ReadError::Parse(
                ParseError::Bad(_) | ParseError::HeadTooLarge
            ))
        ));
    }

    #[test]
    fn responses_carry_the_fixed_headers_and_no_cors() {
        let r = Response::html(200, b"<p>x</p>").to_bytes();
        let text = String::from_utf8(r).expect("ascii");
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        for h in [
            "Connection: close",
            "Cache-Control: no-store",
            "X-Content-Type-Options: nosniff",
            "Referrer-Policy: no-referrer",
            "Content-Security-Policy: default-src 'none'",
            "Content-Length: 8",
        ] {
            assert!(text.contains(h), "{h}");
        }
        assert!(!text.to_ascii_lowercase().contains("access-control"));
        let j =
            String::from_utf8(Response::error(404, "not_found", "x").to_bytes()).expect("ascii");
        assert!(!j.contains("Content-Security-Policy"));
        assert!(j.contains("application/json"));
    }

    #[test]
    fn header_values_with_control_characters_are_dropped() {
        let r = Response::empty(302).header("Location", "/\r\nX: y".to_owned());
        assert!(r.header_value("Location").is_none());
    }

    /// A client that takes `per_call` bytes per `write` and needs `delay`
    /// for each (a slow reader behind a small TCP window).
    struct Trickle {
        out: Vec<u8>,
        per_call: usize,
        delay: Duration,
        limits: Vec<Duration>,
    }

    impl Write for Trickle {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            std::thread::sleep(self.delay);
            let n = buf.len().min(self.per_call);
            self.out.extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Sink for Trickle {
        fn set_write_limit(&mut self, d: Duration) -> io::Result<()> {
            self.limits.push(d);
            Ok(())
        }
    }

    /// SR-01: the write limit covers the whole response, not each call.
    #[test]
    fn a_slow_reader_cannot_hold_the_loop_past_the_write_deadline() {
        let body = vec![b'x'; 64 * 1024];
        let mut slow = Trickle {
            out: Vec::new(),
            per_call: 16,
            delay: Duration::from_millis(2),
            limits: Vec::new(),
        };
        let start = Instant::now();
        let r = write_response(&mut slow, &body, deadline(150));
        let took = start.elapsed();
        assert_eq!(r.map_err(|e| e.kind()), Err(io::ErrorKind::TimedOut));
        // Each call made progress, so a per-call timeout alone would have
        // taken 4096 calls (over 8 s here).
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert!(slow.out.len() < body.len());
        // The socket limit shrinks with the time left.
        assert!(slow.limits.windows(2).all(|w| w[1] <= w[0]));
        assert!(slow.limits.iter().all(|d| *d <= Duration::from_millis(150)));

        let mut fast = Trickle {
            out: Vec::new(),
            per_call: 4096,
            delay: Duration::ZERO,
            limits: Vec::new(),
        };
        write_response(&mut fast, &body, deadline(3000)).expect("written");
        assert_eq!(fast.out, body);
    }
}
