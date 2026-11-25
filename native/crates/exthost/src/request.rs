//! platform/request (request.ts, requestService.ts): HTTP requests of the workbench, here through NSURLSession,
//! which takes the system proxy settings as Electron's `net` module does.
//! shortcut: the `http.proxy*` settings are not applied; port them through the session's proxy dictionary when needed.

use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, bail};
use block2::RcBlock;
use objc2_foundation::{NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURL, NSURLResponse, NSURLSession};

pub struct Request<'a> {
    pub url: &'a str,
    pub method: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    pub timeout: Duration,
}

impl<'a> Request<'a> {
    pub fn get(url: &'a str) -> Self {
        Request { url, method: "GET", headers: &[], body: None, timeout: Duration::from_secs(60) }
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    /// `isSuccess`: 2xx, or 1223 (IE's 204).
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status) || self.status == 1223
    }

    /// `hasNoContent`.
    pub fn has_no_content(&self) -> bool {
        self.status == 204
    }

    /// `asJson`: null for 204, an error for other statuses than success.
    pub fn as_json(&self) -> anyhow::Result<serde_json::Value> {
        if !self.is_success() {
            bail!("Server returned {}", self.status);
        }
        if self.has_no_content() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&self.body).map_err(|error| anyhow::anyhow!("{error}:\n{}", String::from_utf8_lossy(&self.body)))
    }

    /// `asTextOrError`.
    pub fn as_text(&self) -> anyhow::Result<Option<String>> {
        if !self.is_success() {
            bail!("Server returned {}", self.status);
        }
        if self.has_no_content() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&self.body).into_owned()))
    }
}

/// Sends the request and blocks the calling thread until the whole body arrived. NSURLSession decodes gzip.
pub fn request(options: &Request) -> anyhow::Result<Response> {
    let url = NSURL::URLWithString(&NSString::from_str(options.url)).with_context(|| format!("invalid URL {}", options.url))?;
    let request = NSMutableURLRequest::requestWithURL(&url);
    request.setHTTPMethod(&NSString::from_str(options.method));
    request.setTimeoutInterval(options.timeout.as_secs_f64());
    for (name, value) in options.headers {
        request.setValue_forHTTPHeaderField(Some(&NSString::from_str(value)), &NSString::from_str(name));
    }
    if let Some(body) = options.body {
        request.setHTTPBody(Some(&NSData::with_bytes(body)));
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = RcBlock::new(move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
        // SAFETY: NSURLSession passes valid objects or null.
        let result = unsafe {
            match (response.as_ref(), error.as_ref()) {
                (_, Some(error)) => Err(error.localizedDescription().to_string()),
                (Some(response), None) => {
                    let status = response.downcast_ref::<NSHTTPURLResponse>().map_or(200, |response| response.statusCode() as u16);
                    Ok(Response { status, body: data.as_ref().map(|data| data.to_vec()).unwrap_or_default() })
                }
                (None, None) => Err("no response".to_owned()),
            }
        };
        let _ = sender.send(result);
    });
    // SAFETY: the completion block only moves owned data into a channel, so it may run on the session's queue.
    let task = unsafe { NSURLSession::sharedSession().dataTaskWithRequest_completionHandler(&request, &completion) };
    task.resume();
    let result = receiver.recv().context("the request was dropped")?;
    result.map_err(|error| anyhow::anyhow!(error))
}

/// `IDownloadService.download`: the body of a successful GET written to `target`.
pub fn download(url: &str, target: &std::path::Path) -> anyhow::Result<()> {
    let response = request(&Request::get(url))?;
    if response.status != 200 {
        bail!("Expected 200, got back {} instead.\n\n{}", response.status, String::from_utf8_lossy(&response.body));
    }
    Ok(std::fs::write(target, response.body)?)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// One local HTTP exchange: answers `reply` and hands back the raw request.
    fn serve_once(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/path", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                received.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&received);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length: ").map(|value| value.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&received).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn posts_with_headers_and_reads_json() {
        let (url, server) = serve_once("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"a\": true}");
        let response = request(&Request { url: &url, method: "POST", headers: &[("X-Market-Client-Id", "VSCode 1.147.0")], body: Some(b"{}"), timeout: Duration::from_secs(10) }).unwrap();
        assert_eq!(response.as_json().unwrap(), serde_json::json!({ "a": true }));
        let received = server.join().unwrap();
        assert!(received.starts_with("POST /path"));
        assert!(received.contains("X-Market-Client-Id: VSCode 1.147.0"));
        assert!(received.ends_with("{}"));
    }

    #[test]
    fn error_status_is_not_json() {
        let (url, server) = serve_once("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        let response = request(&Request::get(&url)).unwrap();
        assert_eq!(response.status, 404);
        assert_eq!(response.as_json().unwrap_err().to_string(), "Server returned 404");
        server.join().unwrap();
    }

    #[test]
    fn no_content_is_null() {
        let (url, server) = serve_once("HTTP/1.1 204 No Content\r\n\r\n");
        let response = request(&Request::get(&url)).unwrap();
        assert_eq!(response.as_json().unwrap(), serde_json::Value::Null);
        assert_eq!(response.as_text().unwrap(), None);
        server.join().unwrap();
    }
}
