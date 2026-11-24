//! A server process driven line by line.

#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{Receiver, channel},
    thread::JoinHandle,
    time::Duration,
};

use serde_json::{Value as Json, json};

/// How long any one answer may take before the test fails.
pub const PATIENCE: Duration = Duration::from_secs(30);

pub struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Option<JoinHandle<String>>,
    next_id: u64,
}

impl Server {
    pub fn start(binary: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(binary)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the server binary starts");
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut stderr = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
            stderr: Some(stderr),
            next_id: 0,
        }
    }

    /// Writes one line, adding the newline.
    pub fn send(&mut self, line: &str) {
        self.send_bytes(line.as_bytes());
    }
    pub fn send_bytes(&mut self, line: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        stdin.write_all(line).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    /// Writes bytes with no newline added.
    pub fn send_unterminated(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    /// The next line the server wrote, as text.
    pub fn line(&mut self) -> String {
        self.lines
            .recv_timeout(PATIENCE)
            .expect("the server answers in time")
    }
    /// The next line the server wrote, parsed.
    pub fn response(&mut self) -> Json {
        let line = self.line();
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line}"))
    }

    /// Sends one request with a fresh id and returns the whole response.
    pub fn call(&mut self, method: &str, params: Json) -> Json {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string());
        let response = self.response();
        assert_eq!(response["jsonrpc"], "2.0", "{response}");
        assert_eq!(response["id"], id, "{response}");
        response
    }
    /// Sends one request and returns its result, failing on an error.
    pub fn ok(&mut self, method: &str, params: Json) -> Json {
        let mut response = self.call(method, params);
        assert!(response.get("error").is_none(), "{method}: {response}");
        response["result"].take()
    }
    /// Sends one request and returns its error object, failing on a result.
    pub fn err(&mut self, method: &str, params: Json) -> Json {
        let mut response = self.call(method, params);
        assert!(response.get("result").is_none(), "{method}: {response}");
        response["error"].take()
    }
    /// Sends one request that must fail as a Factlite error with this code,
    /// and returns the Factlite error object.
    pub fn refused(&mut self, method: &str, params: Json, code: &str) -> Json {
        let mut error = self.err(method, params);
        assert_eq!(error["code"], -32000, "{error}");
        assert_eq!(error["data"]["code"], code, "{error}");
        assert_eq!(error["message"], error["data"]["message"], "{error}");
        error["data"].take()
    }

    /// Opens a store and returns its handle.
    pub fn open(&mut self, path: &Path, extra: Json) -> String {
        let mut params = json!({"path": path});
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        self.ok("open", params)["handle"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Closes stdin and waits for the process to end. Returns its status,
    /// every line it wrote that was not yet read, and its stderr.
    pub fn finish(mut self) -> (ExitStatus, Vec<String>, String) {
        drop(self.stdin.take());
        let status = self.wait();
        let rest = self.lines.iter().collect();
        let stderr = self.stderr.take().unwrap().join().unwrap();
        (status, rest, stderr)
    }
    /// Waits for the process to end without closing stdin.
    pub fn wait(&mut self) -> ExitStatus {
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the server did not exit in time"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// Kills the process at once, as a crash would.
    pub fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn parse(text: &str) -> Json {
    serde_json::from_str(text).unwrap()
}
