//! TLS for WebDesk's own listener.
//!
//! WebDesk asks for a password and then hands out a terminal, so the default
//! has to be https -- a login form on plain http puts a root-capable shell one
//! passive listener away. Plaintext is still reachable with `WD_TLS=off` for a
//! host that already has a reverse proxy terminating TLS in front of it.
//!
//! The certificate is self-signed unless one is supplied. That is not a
//! ceremony: a self-signed certificate on a LAN address is exactly as
//! authenticated as no certificate at all, and enormously more private. It
//! costs the browser's interstitial once per host. Point `WD_TLS_CERT` and
//! `WD_TLS_KEY` at a real pair and the warning goes away.
//!
//! rustls, not OpenSSL, and `ring`, not aws-lc-rs -- the same reasoning as the
//! proxy's client side in `proxy.rs`: this program has to build on a stock host
//! with nothing beyond gcc, so no OpenSSL headers and no cmake.

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::{server::TlsStream, TlsAcceptor};

/// How long a connection may take to get through the TLS handshake. A client
/// that opens a socket and says nothing is otherwise a free file descriptor
/// held forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling on the request head read from a plaintext caller before answering
/// with a redirect. Nothing legitimate arrives here; this only has to be big
/// enough to find the request line and `Host`.
const PLAINTEXT_PEEK: usize = 4096;

// ------------------------------------------------------------------ listener

/// A TCP listener that hands axum already-negotiated TLS streams.
///
/// Handshakes run in their own tasks and finished connections arrive over a
/// channel, so one client stalling mid-handshake cannot hold up `accept` for
/// everybody else -- which is what would happen if the handshake were awaited
/// inline the way [`axum::serve::Listener`] is otherwise written.
pub struct Listener {
    local: SocketAddr,
    rx: tokio::sync::mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
}

impl axum::serve::Listener for Listener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(pair) => pair,
            // The accept task never returns of its own accord, so an empty
            // channel means it panicked and nothing will ever be served again.
            // Better to die and let systemd restart than to sit there looking
            // healthy while listening to nobody.
            None => {
                tracing::error!("tls accept task died");
                std::process::exit(1);
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Bind `addr` and start accepting TLS connections on it.
pub async fn bind(addr: &str, config: ServerConfig) -> io::Result<Listener> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let acceptor = TlsAcceptor::from(Arc::new(config));

    // Bounded: the queue is finished handshakes waiting for axum to pick them
    // up, and axum picks them up immediately. A backlog here would only mean
    // memory spent on connections nobody is reading yet.
    let (tx, rx) = tokio::sync::mpsc::channel(64);

    tokio::spawn(async move {
        loop {
            let (sock, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    // Per-connection failures (a client that vanished between
                    // SYN and accept, a momentarily exhausted fd table) must
                    // not take the listener down. Pause on the ones that would
                    // otherwise spin.
                    tracing::warn!("accept failed: {e}");
                    if is_fatal_accept(&e) {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    continue;
                }
            };
            let acceptor = acceptor.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                match tokio::time::timeout(HANDSHAKE_TIMEOUT, greet(&acceptor, sock)).await {
                    Ok(Ok(stream)) => {
                        let _ = tx.send((stream, peer)).await;
                    }
                    Ok(Err(e)) => tracing::debug!(%peer, "handshake failed: {e}"),
                    Err(_) => tracing::debug!(%peer, "handshake timed out"),
                }
            });
        }
    });

    Ok(Listener { local, rx })
}

/// Decide what a fresh connection is, and either finish the handshake or send
/// a plaintext caller to the https URL it meant.
///
/// Someone will type `http://host:61443` -- the port is the address they were
/// given, and http is the scheme fingers produce. Without this they get
/// rustls' "invalid message received" and a browser page that says nothing.
async fn greet(acceptor: &TlsAcceptor, mut sock: TcpStream) -> io::Result<TlsStream<TcpStream>> {
    // A TLS ClientHello is a handshake record: first byte 0x16. Every HTTP
    // method starts with an uppercase letter. Peeked, not read, so the byte is
    // still there for rustls.
    let mut first = [0u8; 1];
    if sock.peek(&mut first).await? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if first[0] == 0x16 {
        return acceptor.accept(sock).await;
    }

    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while head.len() < PLAINTEXT_PEEK {
        let n = sock.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let reply = plaintext_reply(&String::from_utf8_lossy(&head));
    sock.write_all(reply.as_bytes()).await?;
    sock.flush().await?;
    Err(io::Error::new(io::ErrorKind::InvalidData, "plaintext request on the tls port"))
}

/// Whether an accept error is the kind that will keep firing instantly.
fn is_fatal_accept(e: &io::Error) -> bool {
    !matches!(
        e.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::NotConnected
    )
}

// ------------------------------------------------- the plaintext consolation

/// The answer to an http request that arrived on the https port: a redirect to
/// the same URL over https when the request said which host it meant, and an
/// explanation otherwise.
///
/// The `Host` and target are copied into a response header, so both are
/// checked rather than trusted -- a header injected here would be reflected to
/// whoever else the client can be talked into visiting.
fn plaintext_reply(head: &str) -> String {
    let mut lines = head.split("\r\n");
    let target = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .filter(|t| t.starts_with('/') && t.len() < 2048 && t.chars().all(is_target_char));
    let host = lines
        .find(|l| l.len() > 5 && l[..5].eq_ignore_ascii_case("host:"))
        .map(|l| l[5..].trim())
        .filter(|h| !h.is_empty() && h.len() < 256 && h.chars().all(is_host_char));

    match (host, target) {
        (Some(host), Some(target)) => format!(
            "HTTP/1.1 308 Permanent Redirect\r\n\
             Location: https://{host}{target}\r\n\
             Content-Length: 0\r\n\
             Connection: close\r\n\r\n"
        ),
        _ => {
            let body = "This port speaks HTTPS. Try the same address with https://\n";
            format!(
                "HTTP/1.1 400 Bad Request\r\n\
                 Content-Type: text/plain; charset=utf-8\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            )
        }
    }
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '_')
}

fn is_target_char(c: char) -> bool {
    c.is_ascii_graphic() && c != '"' && c != '<' && c != '>' && c != '\\' && c != '^'
}

// ----------------------------------------------------------- the certificate

/// Build the server config, from a supplied certificate if there is one and
/// from a self-signed one kept on disk otherwise.
pub fn config() -> Result<ServerConfig, String> {
    // Empty is unset: the unit file always writes both names, and systemd's
    // `Environment=WD_TLS_CERT=` sets an empty value rather than nothing.
    let named = |k| std::env::var_os(k).filter(|v| !v.is_empty());
    let supplied = (named("WD_TLS_CERT"), named("WD_TLS_KEY"));
    let (cert_pem, key_pem, where_from) = match supplied {
        (Some(c), Some(k)) => {
            let (c, k) = (PathBuf::from(c), PathBuf::from(k));
            let cert = read(&c)?;
            let key = read(&k)?;
            (cert, key, format!("{}", c.display()))
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err("WD_TLS_CERT and WD_TLS_KEY must be set together".into())
        }
        (None, None) => {
            let dir = self_signed_dir();
            let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
            match (read(&cert_path), read(&key_path)) {
                (Ok(c), Ok(k)) => (c, k, format!("{}", cert_path.display())),
                _ => {
                    let (c, k) = self_signed()?;
                    match store(&dir, &cert_path, &key_path, &c, &k) {
                        Ok(()) => (c, k, format!("{} (generated)", cert_path.display())),
                        Err(e) => {
                            // Running from a checkout as an ordinary user, most
                            // likely. Serve anyway; the cost is that the
                            // browser meets a new certificate every restart.
                            tracing::warn!("could not save a certificate in {}: {e}", dir.display());
                            (c, k, "memory (not saved)".into())
                        }
                    }
                }
            }
        }
    };

    tracing::info!("tls certificate: {where_from}");
    build(&cert_pem, &key_pem)
}

/// Turn a PEM pair into the config the listener serves.
fn build(cert_pem: &str, key_pem: &str) -> Result<ServerConfig, String> {
    let certs: Vec<CertificateDer<'static>> = pem_blocks(cert_pem)
        .into_iter()
        .filter(|(label, _)| label == "CERTIFICATE")
        .map(|(_, der)| CertificateDer::from(der))
        .collect();
    if certs.is_empty() {
        return Err("no CERTIFICATE block in the certificate file".into());
    }
    let key = private_key(key_pem)?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate and key do not go together: {e}"))?;
    // HTTP/1.1 only, deliberately. The terminal is a websocket, and a websocket
    // over h2 needs extended CONNECT, which is not what this serves. Offering
    // h2 here would let a browser negotiate its way out of a working terminal.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// Where a generated certificate lives: beside the rest of the service state,
/// which the unit file already creates 0700 and root-owned.
fn self_signed_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("WD_TLS_DIR") {
        return PathBuf::from(d);
    }
    let state = std::env::var("WD_STATE_DIR").unwrap_or_else(|_| "/var/lib/webdesk".into());
    Path::new(&state).join("tls")
}

/// Mint a certificate for this host: its own name, `localhost`, and the two
/// loopback addresses. Not for any address the host might also answer on --
/// a certificate nothing verifies gains nothing from listing more names, and
/// the browser's one-time interstitial is unavoidable either way.
fn self_signed() -> Result<(String, String), String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string(), "::1".to_string()];
    if let Ok(h) = nix::unistd::gethostname() {
        if let Some(h) = h.to_str() {
            if !h.is_empty() && h != "localhost" {
                names.insert(0, h.to_string());
            }
        }
    }
    let key = rcgen::generate_simple_self_signed(names)
        .map_err(|e| format!("could not generate a certificate: {e}"))?;
    Ok((key.cert.pem(), key.key_pair.serialize_pem()))
}

/// Write the pair down, with the key readable only by the user that made it.
fn store(dir: &Path, cert: &Path, key: &Path, cert_pem: &str, key_pem: &str) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    std::fs::write(cert, cert_pem)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(key)?;
    io::Write::write_all(&mut f, key_pem.as_bytes())
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Pick the private key out of a PEM file.
///
/// Three labels are in circulation and a supplied key could be any of them:
/// PKCS#8 (`PRIVATE KEY`, what we generate), PKCS#1 (`RSA PRIVATE KEY`) and
/// SEC1 (`EC PRIVATE KEY`). rustls wants to be told which.
fn private_key(pem: &str) -> Result<PrivateKeyDer<'static>, String> {
    for (label, der) in pem_blocks(pem) {
        return Ok(match label.as_str() {
            "PRIVATE KEY" => PrivateKeyDer::try_from(der).map_err(|e| e.to_string())?,
            "RSA PRIVATE KEY" => PrivateKeyDer::Pkcs1(der.into()),
            "EC PRIVATE KEY" => PrivateKeyDer::Sec1(der.into()),
            _ => continue,
        });
    }
    Err("no private key block in the key file".into())
}

/// The three lines of PEM this needs, rather than a crate for it.
///
/// Everything outside the BEGIN/END markers is ignored, which is what lets a
/// file carry a comment header or a certificate chain.
fn pem_blocks(text: &str) -> Vec<(String, Vec<u8>)> {
    use base64::Engine;

    let mut out = Vec::new();
    let mut label: Option<String> = None;
    let mut body = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("-----BEGIN ") {
            label = rest.strip_suffix("-----").map(|l| l.to_string());
            body.clear();
        } else if let Some(rest) = line.strip_prefix("-----END ") {
            let end = rest.strip_suffix("-----").unwrap_or_default();
            if let Some(l) = label.take() {
                if l == end {
                    if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(&body) {
                        out.push((l, der));
                    }
                }
            }
            body.clear();
        } else if label.is_some() {
            body.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plaintext_request_is_sent_to_the_https_url() {
        let reply = plaintext_reply("GET /apps?x=1 HTTP/1.1\r\nHost: desk.example:61443\r\n\r\n");
        assert!(reply.starts_with("HTTP/1.1 308 "), "{reply}");
        assert!(reply.contains("Location: https://desk.example:61443/apps?x=1\r\n"), "{reply}");
    }

    #[test]
    fn a_request_with_no_host_gets_told_why() {
        let reply = plaintext_reply("GET / HTTP/1.0\r\n\r\n");
        assert!(reply.starts_with("HTTP/1.1 400 "), "{reply}");
        assert!(reply.contains("HTTPS"), "{reply}");
    }

    #[test]
    fn a_header_cannot_be_injected_through_the_redirect() {
        // Both halves of the Location come off the wire, so both are checked.
        // Either one rejected means the redirect is not built at all.
        for head in [
            "GET / HTTP/1.1\r\nHost: evil\rSet-Cookie: a=b\r\n\r\n",
            "GET /\u{0}x HTTP/1.1\r\nHost: good.example\r\n\r\n",
            "GET http://elsewhere/ HTTP/1.1\r\nHost: good.example\r\n\r\n",
        ] {
            let reply = plaintext_reply(head);
            assert!(reply.starts_with("HTTP/1.1 400 "), "{head:?} produced {reply}");
        }
    }

    #[test]
    fn the_generated_certificate_is_one_rustls_will_serve() {
        let (cert, key) = self_signed().unwrap();
        let certs: Vec<_> = pem_blocks(&cert)
            .into_iter()
            .map(|(_, der)| CertificateDer::from(der))
            .collect();
        assert_eq!(certs.len(), 1);
        let key = private_key(&key).unwrap();
        ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key).unwrap();
    }

    #[test]
    fn a_pem_file_can_carry_a_chain_and_a_comment() {
        let (cert, _) = self_signed().unwrap();
        let doubled = format!("# leaf, then issuer\n{cert}{cert}");
        assert_eq!(pem_blocks(&doubled).len(), 2);
        assert!(pem_blocks(&doubled).iter().all(|(l, _)| l == "CERTIFICATE"));
    }

    /// The real listener, on a throwaway certificate, serving one route.
    /// Returns the port it ended up on.
    async fn serving() -> u16 {
        let (cert, key) = self_signed().unwrap();
        let listener = bind("127.0.0.1:0", build(&cert, &key).unwrap()).await.unwrap();
        let port = axum::serve::Listener::local_addr(&listener).unwrap().port();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "served" }));
        tokio::spawn(async move { axum::serve(listener, app).await });
        port
    }

    #[tokio::test]
    async fn a_request_over_tls_is_answered() {
        let port = serving().await;
        // The same unverifying dial the proxy uses for a container app: the
        // certificate here is self-signed too, and nothing could verify it.
        let mut c = crate::proxy::dial(port, true).await.expect("handshake failed");
        c.write_all(b"GET / HTTP/1.1\r\nHost: desk.example\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        c.flush().await.unwrap();

        let mut got = String::new();
        c.read_to_string(&mut got).await.unwrap();
        assert!(got.starts_with("HTTP/1.1 200 "), "{got}");
        assert!(got.ends_with("served"), "{got}");
    }

    #[tokio::test]
    async fn a_plaintext_request_on_the_tls_port_gets_a_redirect_not_a_hang() {
        let port = serving().await;
        let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(b"GET /files HTTP/1.1\r\nHost: desk.example\r\n\r\n").await.unwrap();

        let mut got = String::new();
        tokio::time::timeout(Duration::from_secs(10), c.read_to_string(&mut got))
            .await
            .expect("plaintext request hung")
            .unwrap();
        assert!(got.starts_with("HTTP/1.1 308 "), "{got}");
        assert!(got.contains("Location: https://desk.example/files\r\n"), "{got}");
    }

    #[tokio::test]
    async fn a_client_that_says_nothing_does_not_hold_the_door() {
        // The handshake timeout is what keeps a silent connection from owning
        // a file descriptor forever; that it is per-connection is what keeps it
        // from owning the listener. Only the second half is quick to observe.
        let port = serving().await;
        let _silent = TcpStream::connect(("127.0.0.1", port)).await.unwrap();

        let mut c = tokio::time::timeout(Duration::from_secs(10), crate::proxy::dial(port, true))
            .await
            .expect("a silent client blocked the listener")
            .expect("handshake failed");
        c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
        c.flush().await.unwrap();
        let mut got = String::new();
        c.read_to_string(&mut got).await.unwrap();
        assert!(got.starts_with("HTTP/1.1 200 "), "{got}");
    }

    #[test]
    fn a_truncated_pem_block_is_not_half_read() {
        let (cert, _) = self_signed().unwrap();
        let cut = &cert[..cert.len() / 2];
        assert!(pem_blocks(cut).is_empty());
    }
}
