//! The reverse proxy that puts a container app on WebDesk's own origin.
//!
//! Every installed app is published on a loopback port and reachable only
//! through `/app/<slug>/` here. That single decision is what makes the rest
//! work:
//!
//! - **It is the access control.** Containers bind `127.0.0.1`, so this handler
//!   is the only route to them, and it refuses anyone without a session. An app
//!   with no login of its own is still not open to the network.
//! - **It is why the iframe works.** Same origin means the browser raises no
//!   objection, and `X-Frame-Options` from the app is ours to drop -- it is
//!   advice to the browser about a page we are serving, not a claim about a
//!   site we do not control.
//! - **It is why there is one port to open.** The firewall story stays exactly
//!   what it was: the one WebDesk listens on, and nothing else. The single
//!   exception is an entry that sets `needs_origin` and cannot be served from a
//!   prefix at all -- see `origin.rs`, which is deliberately a separate module
//!   so that the cost of that choice is not spread through this one.
//! - **It is why every app is on https.** The browser's connection is to
//!   WebDesk, which now terminates TLS itself (see `tls.rs`), so an app that
//!   speaks plaintext to this proxy over loopback still reaches the user
//!   encrypted -- and inside a secure context, which is what the desktop
//!   images need for the clipboard and for audio.
//!
//! Two rules are load-bearing rather than cosmetic, and both are about keeping
//! an app from reaching past its prefix: the session cookie is stripped on the
//! way in so an app never sees it, and cookies coming back are pinned to the
//! app's own path, with any attempt to set `wd_session` dropped outright.
//!
//! A fresh connection is made per request rather than pooled. Over loopback a
//! connect costs microseconds, and doing it this way means upgrades and plain
//! requests take exactly the same path through this file.

use crate::{apps, session_of, unauthorized, AppState, COOKIE};
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;

/// Headers that describe one hop and must not be forwarded to the next.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

/// Response headers the app does not get a say in, because they are about the
/// page WebDesk is serving rather than about the app.
const DROPPED_RESPONSE: &[&str] = &["x-frame-options"];

pub async fn handle_root(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    // Land on the trailing slash first. Without it every relative link in the
    // app resolves one level too high and the page comes back bare.
    let Path(slug) = &path;
    if session_of(&state, &headers).is_none() {
        return unauthorized();
    }
    if apps::upstream_of(slug).is_none() {
        return not_installed(slug);
    }
    let q = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, format!("/app/{slug}/{q}"))],
    )
        .into_response()
}

/// `/app/<slug>/` -- the app's own root, and the URL the iframe actually loads.
///
/// It needs a route of its own because a wildcard segment matches one path
/// segment or more, never the empty one, so this exact URL would otherwise fall
/// through to the static handler and 404.
pub async fn handle_index(
    state: State<AppState>,
    path: Path<String>,
    req: Request,
) -> Response {
    let Path(slug) = path;
    proxy_to(state, slug, String::new(), req).await
}

pub async fn handle(
    state: State<AppState>,
    Path((slug, rest)): Path<(String, String)>,
    req: Request,
) -> Response {
    proxy_to(state, slug, rest, req).await
}

async fn proxy_to(
    State(state): State<AppState>,
    slug: String,
    rest: String,
    req: Request,
) -> Response {
    if session_of(&state, req.headers()).is_none() {
        return unauthorized();
    }
    let Some((port, tls)) = apps::upstream_of(&slug) else {
        return not_installed(&slug);
    };
    forward(port, tls, &format!("/app/{slug}"), &slug, rest, req).await
}

/// Open the connection to an app, in plaintext or TLS.
///
/// **The certificate is not checked, and checking it would mean nothing.** The
/// desktop images generate their own self-signed certificate with `CN=*` at
/// first start; there is no authority to verify it against and no name to match
/// it to. What makes this hop private is that it is a loopback socket to a port
/// bound on `127.0.0.1` -- the TLS is a requirement of the port, not a security
/// boundary WebDesk relies on. Nothing here ever reaches the network, so this
/// verifier cannot be tricked by anything that is not already on the machine.
pub(crate) async fn dial(port: u16, tls: bool) -> std::io::Result<Box<dyn Upstream>> {
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    if !tls {
        return Ok(Box::new(tcp));
    }

    use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified};
    use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

    #[derive(Debug)]
    struct AnyCert;

    impl tokio_rustls::rustls::client::danger::ServerCertVerifier for AnyCert {
        fn verify_server_cert(
            &self,
            _: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
            _: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
            _: &tokio_rustls::rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: tokio_rustls::rustls::pki_types::UnixTime,
        ) -> Result<ServerCertVerified, tokio_rustls::rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            // Everything ring implements; the verifier accepts them all anyway.
            vec![
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::RSA_PKCS1_SHA384,
                SignatureScheme::RSA_PKCS1_SHA512,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::ECDSA_NISTP384_SHA384,
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::RSA_PSS_SHA384,
                SignatureScheme::RSA_PSS_SHA512,
                SignatureScheme::ED25519,
            ]
        }
    }

    // Built once. The handshake is on the path of every request to a desktop
    // app, and rebuilding the config per connection would redo all of this.
    static CONFIG: std::sync::OnceLock<std::sync::Arc<ClientConfig>> = std::sync::OnceLock::new();
    let config = CONFIG
        .get_or_init(|| {
            let c = ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(std::sync::Arc::new(AnyCert))
                .with_no_client_auth();
            std::sync::Arc::new(c)
        })
        .clone();

    let name = tokio_rustls::rustls::pki_types::ServerName::IpAddress(
        std::net::Ipv4Addr::LOCALHOST.into(),
    );
    let stream = tokio_rustls::TlsConnector::from(config).connect(name, tcp).await?;
    Ok(Box::new(stream))
}

/// Either half of what `dial` can return, so `forward` does not care which.
pub trait Upstream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> Upstream for T {}

/// Everything after "which app, and may this session have it": open a
/// connection, relay the request, and relay the answer back with the
/// rewriting applied. Split out from the routing so it can be tested against a
/// real upstream without needing a session to exist.
pub(crate) async fn forward(
    port: u16,
    tls: bool,
    prefix: &str,
    slug: &str,
    rest: String,
    mut req: Request,
) -> Response {
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    // Origin-form -- a path, not a URL. The connection is already open to the
    // right place, so the absolute form would be the one a forward proxy sends,
    // and an origin server is entitled to reject it. Several of the images in
    // the catalog are nginx underneath and do exactly that.
    let upstream_uri: Uri = match format!("/{rest}{query}").parse() {
        Ok(u) => u,
        Err(e) => return oops(StatusCode::BAD_REQUEST, format!("bad path: {e}")),
    };

    let upgrade = wants_upgrade(req.headers());
    let client_upgrade = req.extensions_mut().remove::<OnUpgrade>();

    // ---- build the upstream request
    let mut out = hyper::Request::builder().method(req.method().clone()).uri(upstream_uri);
    {
        let h = out.headers_mut().unwrap();
        for (name, value) in req.headers() {
            if HOP_BY_HOP.contains(&name.as_str()) {
                continue;
            }
            if name == header::COOKIE {
                if let Some(v) = strip_session_cookie(value) {
                    h.insert(header::COOKIE, v);
                }
                continue;
            }
            h.append(name.clone(), value.clone());
        }

        // Tell the app where it is. Anything that honours X-Forwarded-Prefix
        // needs no further configuration; anything that does not was given the
        // same string as an environment variable at install time.
        // Empty when the app owns its origin, and an empty prefix is not a
        // fact worth sending: an app that reads the header would take it as
        // "you are mounted at nothing" rather than "you are at the root".
        if !prefix.is_empty() {
            let _ = HeaderValue::from_str(&prefix).map(|v| h.insert("x-forwarded-prefix", v));
        }
        let scheme = if req.uri().scheme_str() == Some("https") {
            HeaderValue::from_static("https")
        } else {
            HeaderValue::from_static("http")
        };
        h.insert("x-forwarded-proto", scheme);
        match req.headers().get(header::HOST).cloned() {
            // The app is told the name the browser used, not the loopback
            // address it happens to listen on, so anything building an
            // absolute URL from Host builds a reachable one.
            Some(host) => {
                h.insert("x-forwarded-host", host);
            }
            // HTTP/1.1 requires a Host, and a request that arrived without one
            // would otherwise be refused by the upstream rather than by us.
            None => {
                if let Ok(v) = HeaderValue::from_str(&format!("127.0.0.1:{port}")) {
                    h.insert(header::HOST, v);
                }
            }
        }

        if upgrade {
            h.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
            if let Some(v) = req.headers().get(header::UPGRADE).cloned() {
                h.insert(header::UPGRADE, v);
            }
        }
    }

    let out = match out.body(req.into_body()) {
        Ok(r) => r,
        Err(e) => return oops(StatusCode::INTERNAL_SERVER_ERROR, e),
    };

    // ---- one connection, one request
    let stream = match dial(port, tls).await {
        Ok(s) => s,
        Err(e) => return unreachable_app(slug, e),
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
    {
        Ok(v) => v,
        Err(e) => return unreachable_app(&slug, e),
    };
    // with_upgrades keeps the connection task alive past a 101 so the tunnel
    // below has something to run on.
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!("app connection ended: {e}");
        }
    });

    let mut upstream = match sender.send_request(out).await {
        Ok(r) => r,
        Err(e) => return unreachable_app(&slug, e),
    };

    // ---- a websocket, or anything else that changed protocols
    if upstream.status() == StatusCode::SWITCHING_PROTOCOLS {
        let server_upgrade = upstream.extensions_mut().remove::<OnUpgrade>();
        if let (Some(client), Some(server)) = (client_upgrade, server_upgrade) {
            tokio::spawn(async move {
                match tokio::try_join!(client, server) {
                    Ok((a, b)) => {
                        let (mut a, mut b) = (TokioIo::new(a), TokioIo::new(b));
                        if let Err(e) = tokio::io::copy_bidirectional(&mut a, &mut b).await {
                            tracing::debug!("app socket closed: {e}");
                        }
                    }
                    Err(e) => tracing::debug!("app socket could not be upgraded: {e}"),
                }
            });
        }
    }

    // ---- hand the response back, minus what the app does not get to decide
    let mut builder = Response::builder().status(upstream.status());
    {
        let h = builder.headers_mut().unwrap();
        let switching = upstream.status() == StatusCode::SWITCHING_PROTOCOLS;
        for (name, value) in upstream.headers() {
            let n = name.as_str();
            if DROPPED_RESPONSE.contains(&n) {
                continue;
            }
            if HOP_BY_HOP.contains(&n) && !switching {
                continue;
            }
            match n {
                "set-cookie" => {
                    if let Some(v) = rewrite_cookie(value, &prefix) {
                        h.append(header::SET_COOKIE, v);
                    }
                }
                "location" => {
                    h.insert(header::LOCATION, rewrite_location(value, &prefix));
                }
                "content-security-policy" | "content-security-policy-report-only" => {
                    if let (Ok(name), Some(v)) =
                        (HeaderName::try_from(n), strip_frame_ancestors(value))
                    {
                        h.insert(name, v);
                    }
                }
                _ => {
                    h.append(name.clone(), value.clone());
                }
            }
        }
    }

    builder.body(Body::new(upstream.into_body())).unwrap_or_else(|e| {
        oops(StatusCode::INTERNAL_SERVER_ERROR, format!("could not relay the response: {e}"))
    })
}

// ---------------------------------------------------------------- rewriting

fn wants_upgrade(h: &HeaderMap) -> bool {
    h.get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")))
        .unwrap_or(false)
        && h.contains_key(header::UPGRADE)
}

/// Drop WebDesk's session cookie from what the app is sent. An app never has a
/// reason to see it, and one that logged its request headers would otherwise be
/// writing session tokens to disk.
fn strip_session_cookie(value: &HeaderValue) -> Option<HeaderValue> {
    let text = value.to_str().ok()?;
    let kept: Vec<&str> = text
        .split(';')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .filter(|c| {
            let name = c.split('=').next().unwrap_or("").trim();
            name != COOKIE
        })
        .collect();
    if kept.is_empty() {
        return None;
    }
    HeaderValue::from_str(&kept.join("; ")).ok()
}

/// Pin a cookie the app sets to the app's own path, and refuse one that would
/// impersonate the session cookie.
///
/// Without the pinning, an app setting `Path=/` would have its cookie sent to
/// WebDesk and to every other app -- which is both a privacy leak between apps
/// and a way for two apps with the same cookie name to log each other out.
fn rewrite_cookie(value: &HeaderValue, prefix: &str) -> Option<HeaderValue> {
    let text = value.to_str().ok()?;
    let name = text.split('=').next().unwrap_or("").trim();
    if name == COOKIE {
        tracing::warn!("an app tried to set {COOKIE}; dropped");
        return None;
    }

    let want = format!("{prefix}/");
    let mut parts: Vec<String> = Vec::new();
    let mut had_path = false;
    for part in text.split(';') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if t.len() >= 5 && t[..5].eq_ignore_ascii_case("path=") {
            had_path = true;
            parts.push(format!("Path={want}"));
        } else {
            parts.push(t.to_string());
        }
    }
    if !had_path {
        parts.push(format!("Path={want}"));
    }
    HeaderValue::from_str(&parts.join("; ")).ok()
}

/// Move a redirect back under the app's prefix. Absolute-path redirects are the
/// common case; a full URL is only touched when it points at the app itself.
fn rewrite_location(value: &HeaderValue, prefix: &str) -> HeaderValue {
    let Ok(text) = value.to_str() else { return value.clone() };

    let moved = if text.starts_with("//") {
        // Protocol-relative, so another host. Leave it.
        text.to_string()
    } else if let Some(rest) = text.strip_prefix('/') {
        format!("{prefix}/{rest}")
    } else if let Some(rest) = text.strip_prefix("http://127.0.0.1") {
        // The app told us its own address. Everything after the port is the path.
        let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
        format!("{prefix}/{path}")
    } else {
        text.to_string()
    };
    HeaderValue::from_str(&moved).unwrap_or_else(|_| value.clone())
}

/// Remove a `frame-ancestors` directive so the app's own policy cannot stop it
/// being framed by the desktop that is serving it. The rest of its policy is
/// left exactly as it was.
fn strip_frame_ancestors(value: &HeaderValue) -> Option<HeaderValue> {
    let text = value.to_str().ok()?;
    let kept: Vec<&str> = text
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .filter(|d| !d.to_ascii_lowercase().starts_with("frame-ancestors"))
        .collect();
    if kept.is_empty() {
        return None;
    }
    HeaderValue::from_str(&kept.join("; ")).ok()
}

// ------------------------------------------------------------------ errors

fn oops(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, axum::Json(serde_json::json!({ "error": msg.to_string() }))).into_response()
}

pub(crate) fn not_installed(slug: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        page("Not installed", &format!("There is no app called “{slug}” on this host.")),
    )
        .into_response()
}

/// The app is installed but nothing answered on its port -- almost always a
/// stopped container. Said in the frame, where the user is looking.
fn unreachable_app(slug: &str, e: impl std::fmt::Display) -> Response {
    tracing::warn!(slug, "could not reach the app: {e}");
    (
        StatusCode::BAD_GATEWAY,
        page(
            "Not running",
            &format!("“{slug}” did not answer. It is probably stopped — open Apps to start it."),
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }
    fn cookie_out(s: &str, prefix: &str) -> Option<String> {
        rewrite_cookie(&hv(s), prefix).map(|v| v.to_str().unwrap().to_string())
    }

    #[test]
    fn session_cookie_never_reaches_the_app() {
        let sent = hv("wd_session=secret; theme=dark");
        let got = strip_session_cookie(&sent).unwrap();
        assert_eq!(got.to_str().unwrap(), "theme=dark");
    }

    #[test]
    fn a_cookie_header_of_only_the_session_is_dropped_entirely() {
        assert!(strip_session_cookie(&hv("wd_session=secret")).is_none());
    }

    #[test]
    fn a_cookie_named_like_ours_is_not_forwarded_but_a_similar_one_is() {
        // Exact name only: wd_session_id is somebody else's cookie.
        assert_eq!(
            strip_session_cookie(&hv("wd_session_id=1; a=2")).unwrap().to_str().unwrap(),
            "wd_session_id=1; a=2"
        );
    }

    #[test]
    fn an_app_cannot_set_the_session_cookie() {
        assert!(cookie_out("wd_session=forged; Path=/", "/app/x").is_none());
    }

    #[test]
    fn an_app_cookie_is_pinned_to_the_app() {
        assert_eq!(
            cookie_out("sid=abc; Path=/; HttpOnly", "/app/x").unwrap(),
            "sid=abc; Path=/app/x/; HttpOnly"
        );
    }

    #[test]
    fn a_cookie_with_no_path_still_gets_one() {
        assert_eq!(cookie_out("sid=abc; HttpOnly", "/app/x").unwrap(), "sid=abc; HttpOnly; Path=/app/x/");
    }

    #[test]
    fn a_path_attribute_is_matched_whatever_its_case() {
        assert_eq!(cookie_out("sid=abc; PATH=/deep/link", "/app/x").unwrap(), "sid=abc; Path=/app/x/");
    }

    #[test]
    fn an_absolute_redirect_moves_under_the_prefix() {
        assert_eq!(
            rewrite_location(&hv("/login?next=/x"), "/app/x").to_str().unwrap(),
            "/app/x/login?next=/x"
        );
    }

    #[test]
    fn a_redirect_to_the_apps_own_address_is_moved_too() {
        assert_eq!(
            rewrite_location(&hv("http://127.0.0.1:47000/setup"), "/app/x").to_str().unwrap(),
            "/app/x/setup"
        );
    }

    #[test]
    fn a_redirect_to_somewhere_else_is_left_alone() {
        for url in ["https://example.com/oauth", "//cdn.example.com/x", "relative/path"] {
            assert_eq!(rewrite_location(&hv(url), "/app/x").to_str().unwrap(), url);
        }
    }

    #[test]
    fn frame_ancestors_goes_and_the_rest_of_the_policy_stays() {
        let got = strip_frame_ancestors(&hv("default-src 'self'; frame-ancestors 'none'; img-src *"))
            .unwrap();
        assert_eq!(got.to_str().unwrap(), "default-src 'self'; img-src *");
    }

    #[test]
    fn a_policy_that_was_only_frame_ancestors_is_dropped() {
        assert!(strip_frame_ancestors(&hv("frame-ancestors 'none'")).is_none());
    }

    /// A one-shot upstream that records the request it was given and answers
    /// with a response shaped like the awkward ones real apps send. Raw bytes
    /// rather than a server framework, so the test depends on nothing the
    /// proxy itself depends on.
    async fn one_shot_upstream() -> (u16, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());

            let body = "hello from the app";
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                 content-type: text/plain\r\n\
                 content-length: {}\r\n\
                 x-frame-options: DENY\r\n\
                 content-security-policy: default-src 'self'; frame-ancestors 'none'\r\n\
                 set-cookie: appsid=xyz; Path=/; HttpOnly\r\n\
                 location: /after-login\r\n\
                 \r\n{body}",
                body.len()
            );
            sock.write_all(response.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });

        (port, rx)
    }

    #[tokio::test]
    async fn an_app_on_its_own_origin_is_relayed_at_the_root() {
        // The whole of `origin.rs` in one call: an empty prefix. This is what
        // makes an app like dockhand work at all -- it asks for /api/containers
        // absolutely, and here that is exactly what the upstream is asked for,
        // rather than being folded under /app/<slug>.
        let (port, seen) = one_shot_upstream().await;

        let req = Request::builder()
            .method("GET")
            .uri("/api/containers?all=true")
            .header(header::COOKIE, "wd_session=secret; appsid=xyz")
            .header(header::HOST, "desk.example:61444")
            .body(Body::empty())
            .unwrap();

        let res = forward(port, false, "", "demo", "api/containers".into(), req).await;
        assert_eq!(res.status(), StatusCode::OK);

        let sent = seen.await.unwrap();
        assert!(
            sent.starts_with("GET /api/containers?all=true HTTP/1.1"),
            "wrong request line in:\n{sent}"
        );
        // The session cookie is still WebDesk's alone, on this route too.
        assert!(!sent.contains("wd_session"), "the session cookie reached the app:\n{sent}");
        assert!(sent.contains("appsid=xyz"), "the app's own cookie was dropped:\n{sent}");
        // An empty prefix is not a fact worth sending. An app that reads the
        // header would take "" as "mounted at nothing", not "at the root".
        assert!(
            !sent.to_lowercase().contains("x-forwarded-prefix"),
            "an empty prefix was announced:\n{sent}"
        );

        // Path=/ is correct here and is not the leak it would be under a
        // prefix: / is all this origin serves.
        let cookie = res.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(cookie.contains("Path=/"), "{cookie}");
        assert!(!cookie.contains("Path=/app"), "{cookie}");
        // And a redirect the app wrote is already right for this origin.
        assert_eq!(res.headers().get(header::LOCATION).unwrap(), "/after-login");
    }

    #[tokio::test]
    async fn a_request_is_relayed_and_the_answer_is_rewritten() {
        let (port, seen) = one_shot_upstream().await;

        let req = Request::builder()
            .method("GET")
            .uri("/app/demo/page?q=1")
            .header(header::COOKIE, "wd_session=secret; appsid=xyz")
            .header(header::HOST, "desk.example:6767")
            .body(Body::empty())
            .unwrap();

        let res = forward(port, false, "/app/demo", "demo", "page".into(), req).await;
        assert_eq!(res.status(), StatusCode::OK);

        // ---- what the app was given
        let sent = seen.await.unwrap();
        assert!(sent.starts_with("GET /page?q=1 HTTP/1.1"), "wrong request line in:\n{sent}");
        assert!(
            !sent.to_lowercase().contains("wd_session"),
            "the session cookie reached the app:\n{sent}"
        );
        assert!(sent.to_lowercase().contains("appsid=xyz"), "the app's own cookie was lost");
        assert!(sent.to_lowercase().contains("x-forwarded-prefix: /app/demo"), "no prefix:\n{sent}");

        // ---- what the browser gets back
        let h = res.headers();
        assert!(h.get("x-frame-options").is_none(), "the app could still refuse to be framed");
        assert_eq!(
            h.get(header::SET_COOKIE).unwrap().to_str().unwrap(),
            "appsid=xyz; Path=/app/demo/; HttpOnly"
        );
        assert_eq!(h.get(header::LOCATION).unwrap().to_str().unwrap(), "/app/demo/after-login");
        assert_eq!(
            h.get("content-security-policy").unwrap().to_str().unwrap(),
            "default-src 'self'"
        );

        let body = axum::body::to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(&body[..], b"hello from the app");
    }

    /// A TLS listener with a self-signed certificate for a name nothing will
    /// ever match -- the same shape the desktop images generate for themselves.
    /// Echoes one line, so a successful handshake is observable.
    async fn self_signed_echo() -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

        let cert = rcgen::generate_simple_self_signed(vec!["not-localhost.invalid".into()]).unwrap();
        let der = CertificateDer::from(cert.cert.der().to_vec());
        let key = PrivateKeyDer::try_from(cert.key_pair.serialize_der()).unwrap();

        let config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![der], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(sock).await.unwrap();
            let mut buf = [0u8; 32];
            let n = tls.read(&mut buf).await.unwrap();
            tls.write_all(&buf[..n]).await.unwrap();
            tls.flush().await.unwrap();
        });
        port
    }

    #[tokio::test]
    async fn a_tls_upstream_is_reached_despite_an_unverifiable_certificate() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let port = self_signed_echo().await;
        // Self-signed, wrong name, no authority -- exactly what the desktop
        // images present, and the handshake has to succeed anyway.
        let mut up = dial(port, true).await.expect("tls handshake failed");
        up.write_all(b"ping").await.unwrap();
        up.flush().await.unwrap();

        let mut buf = [0u8; 4];
        up.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn plaintext_into_a_tls_port_is_an_error_not_a_hang() {
        // The failure mode of getting `tls` wrong in a catalog entry. It must
        // surface as an error rather than sitting there.
        let port = self_signed_echo().await;
        let req = Request::builder().uri("/app/demo/").body(Body::empty()).unwrap();
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            forward(port, false, "/app/demo", "demo", String::new(), req),
        )
        .await
        .expect("forward hung on a TLS port");
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn an_app_that_is_not_listening_says_so_in_the_frame() {
        // Bind and drop, so the port is almost certainly free and refusing.
        let port = {
            let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            l.local_addr().unwrap().port()
        };
        let req = Request::builder().uri("/app/demo/").body(Body::empty()).unwrap();
        let res = forward(port, false, "/app/demo", "demo", String::new(), req).await;

        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
        let ct = res.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap().to_string();
        // HTML, because this failure is only ever seen inside an iframe.
        assert!(ct.starts_with("text/html"), "got {ct}");
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Not running"));
    }

    #[test]
    fn an_upgrade_needs_both_headers() {
        let mut h = HeaderMap::new();
        h.insert(header::CONNECTION, hv("keep-alive, Upgrade"));
        assert!(!wants_upgrade(&h), "no Upgrade header yet");
        h.insert(header::UPGRADE, hv("websocket"));
        assert!(wants_upgrade(&h));
    }
}

/// A whole page rather than a JSON body: what fails here fails inside an
/// iframe, where only HTML is ever seen.
fn page(title: &str, detail: &str) -> Response {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title>\
         <style>html{{color-scheme:dark}}body{{margin:0;height:100vh;display:grid;\
         place-content:center;gap:.4rem;text-align:center;background:#0d1117;color:#e6eaf0;\
         font:15px/1.5 system-ui,sans-serif}}h1{{font-size:1.05rem;margin:0;font-weight:600}}\
         p{{margin:0;color:#93a1b1}}</style><h1>{title}</h1><p>{detail}</p>"
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}
