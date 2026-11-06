//! Test coordinators for a client's tests, here and in other crates (their dev-dependency on
//! coordwire turns on its `testkit` feature): a listener answering every request and one closing
//! each connection unanswered, each counting them, and an address nothing listens at.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A test's listener (store::spawn's, not blocking) and its connections as they come, each
/// blocking again, to `each` on its own thread's loop.
pub fn serve(each: impl Fn(std::net::TcpStream) + Send + 'static) -> String {
    let l = store::spawn::listen("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || loop {
        match store::spawn::accept(&l) {
            Ok((s, _)) => {
                s.set_nonblocking(false).ok();
                each(s);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(2))
            }
            Err(_) => {}
        }
    });
    url
}

/// A coordinator that never answers: each connection taken (counted, before it's closed) and
/// closed without a word, so a request fails at once rather than waiting out a timeout.
pub fn mute() -> (String, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let seen = n.clone();
    let url = serve(move |s| {
        seen.fetch_add(1, Ordering::SeqCst);
        drop(s);
    });
    (url, n)
}

/// A coordinator answering every request with `status` and `{}`, counting them.
pub fn answering(status: u16) -> (String, Arc<AtomicUsize>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let n = Arc::new(AtomicUsize::new(0));
    let seen = n.clone();
    let url = serve(move |mut s| {
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body).ok();
        seen.fetch_add(1, Ordering::SeqCst);
        write!(s, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").ok();
    });
    (url, n)
}

/// An address nothing listens at (a listener's, closed): a connection to it refused.
pub fn dead() -> String {
    let l = store::spawn::listen("127.0.0.1:0").unwrap();
    format!("http://{}", l.local_addr().unwrap())
}
