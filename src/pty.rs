//! Terminal sessions.
//!
//! The shell is started with `su - <user>`, which is the least code that gets
//! the privileges right: su is setuid-aware, runs its own PAM session, sets up
//! the environment and lands in the user's home. The daemon is root, so no
//! password is requested.

use crate::{session_of, AppState};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
#[serde(tag = "t")]
enum Control {
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16 },
}

pub async fn ws_term(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(session) = session_of(&state, &headers) else {
        return (axum::http::StatusCode::UNAUTHORIZED, "not signed in").into_response();
    };
    let username = session.ident.username.clone();
    ws.on_upgrade(move |socket| run(socket, username))
}

fn find_su() -> Option<&'static str> {
    ["/bin/su", "/usr/bin/su"].into_iter().find(|p| std::path::Path::new(p).exists())
}

async fn run(socket: WebSocket, username: String) {
    let (mut tx, mut rx) = socket.split();

    let Some(su) = find_su() else {
        let _ = tx.send(Message::Text("su not found on this system\r\n".into())).await;
        return;
    };

    let pair = match native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(Message::Text(format!("openpty failed: {e}\r\n").into())).await;
            return;
        }
    };

    let mut cmd = CommandBuilder::new(su);
    cmd.arg("-");
    cmd.arg(&username);
    cmd.env("TERM", "xterm-256color");

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Message::Text(format!("failed to start shell: {e}\r\n").into())).await;
            return;
        }
    };
    drop(pair.slave);

    let master = Arc::new(Mutex::new(pair.master));
    let (reader, writer) = {
        let m = master.lock().unwrap();
        match (m.try_clone_reader(), m.take_writer()) {
            (Ok(r), Ok(w)) => (r, w),
            _ => {
                let _ = child.kill();
                return;
            }
        }
    };
    let writer = Arc::new(Mutex::new(writer));

    // PTY -> browser. Blocking reads live on their own thread.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let pump = tokio::spawn(async move {
        while let Some(chunk) = out_rx.recv().await {
            if tx.send(Message::Binary(Bytes::from(chunk))).await.is_err() {
                break;
            }
        }
        let _ = tx.close().await;
    });

    // browser -> PTY
    while let Some(Ok(msg)) = rx.next().await {
        match msg {
            Message::Binary(data) => {
                let w = writer.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    use std::io::Write;
                    let mut w = w.lock().unwrap();
                    let _ = w.write_all(&data);
                    let _ = w.flush();
                })
                .await;
            }
            Message::Text(text) => {
                if let Ok(Control::Resize { cols, rows }) = serde_json::from_str::<Control>(&text) {
                    let m = master.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = m.lock().unwrap().resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    })
                    .await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    pump.abort();
}
