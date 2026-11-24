//! JSON-RPC 2.0 over lines: one message in, at most one line out.

use std::io::{self, BufRead, Write};

use serde_json::value::RawValue;

use crate::{
    dispatch::{Dispatcher, ORDER_DEPENDENT},
    json::{self, Kind, Malformed, Members, Top},
};

/// The default longest accepted line: 16 MiB, not counting the newline.
pub const DEFAULT_MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;

/// Transport settings for [`serve`].
#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// The longest accepted line in bytes, not counting the newline. A
    /// longer line is discarded as it is read and answered with -32700.
    pub max_line_bytes: usize,
}
impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
        }
    }
}

/// One line of input.
pub(crate) enum Line {
    /// End of input with nothing pending.
    End,
    /// A line longer than the limit. Its bytes were dropped.
    TooLong,
    /// The bytes of one line, without the newline.
    Bytes(Vec<u8>),
}

/// Reads one line without ever holding more than `max` bytes of it. A last
/// line with no newline before end of input is still a line.
pub(crate) fn read_line(input: &mut impl BufRead, max: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    let mut too_long = false;
    let mut any = false;
    loop {
        let buffer = match input.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if buffer.is_empty() {
            return Ok(match (too_long, any) {
                (true, _) => Line::TooLong,
                (false, true) => Line::Bytes(line),
                (false, false) => Line::End,
            });
        }
        any = true;
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let chunk = &buffer[..newline.unwrap_or(buffer.len())];
        if !too_long {
            if chunk.len() > max - line.len() {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = newline.map_or(buffer.len(), |index| index + 1);
        input.consume(used);
        if newline.is_some() {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Bytes(line)
            });
        }
    }
}

/// Whether a line holds nothing but JSON whitespace.
pub(crate) fn is_blank(line: &[u8]) -> bool {
    line.iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}

/// A response line for a result.
pub(crate) fn success(id: &str, result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#)
}
/// A response line for an error. `id` and `data` are JSON text.
pub(crate) fn failure(id: &str, code: i64, message: &str, data: Option<&str>) -> String {
    let message = json::quote(message);
    match data {
        None => format!(
            r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":{message}}}}}"#
        ),
        Some(data) => format!(
            r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":{code},"message":{message},"data":{data}}}}}"#
        ),
    }
}

/// One well-formed request object.
pub(crate) struct Request {
    /// The id's text as sent, or None for a notification.
    pub(crate) id: Option<Box<RawValue>>,
    pub(crate) method: String,
    pub(crate) params: Option<Box<RawValue>>,
}

/// Checks the JSON-RPC envelope. The error is the message of a -32600
/// response, whose id is always null.
pub(crate) fn request(mut members: Members) -> Result<Request, String> {
    match members.take("jsonrpc").as_deref().and_then(json::string) {
        Some(version) if version == "2.0" => {}
        _ => return Err("jsonrpc must be \"2.0\"".into()),
    }
    let method = members
        .take("method")
        .as_deref()
        .and_then(json::string)
        .ok_or("method must be a string")?;
    let id = members.take("id");
    if let Some(id) = &id {
        // The id is echoed as sent, so a string must be one that decodes.
        let valid = match json::kind(id) {
            Kind::String => json::string(id).is_some(),
            Kind::Number | Kind::Null => true,
            Kind::Object | Kind::Array | Kind::Bool => false,
        };
        if !valid {
            return Err("id must be a string, a number or null".into());
        }
    }
    let params = members.take("params");
    if let Some(key) = members.remaining() {
        return Err(format!("unknown member: {key}"));
    }
    Ok(Request { id, method, params })
}

/// What one entry of input produced.
enum Outcome {
    /// A response line (without the newline).
    Respond(String),
    /// Nothing to send: a notification.
    Silent,
}

struct Session<'a, L> {
    dispatcher: &'a mut Dispatcher,
    log: L,
    shutdown: bool,
}

impl<L: Write> Session<'_, L> {
    fn note(&mut self, message: &str) {
        // Diagnostics are best effort; a closed stderr must not stop service.
        let _ = writeln!(self.log, "factlite-rpc: {message}");
    }

    /// Handles one request object, alone or as a batch entry.
    fn entry(&mut self, top: Result<Top, Malformed>, in_batch: bool) -> Outcome {
        let invalid =
            |message: &str| Outcome::Respond(failure("null", INVALID_REQUEST, message, None));
        let members = match top {
            Ok(Top::Object(members)) => members,
            Ok(_) => return invalid("a request must be an object"),
            Err(Malformed::Duplicate(message)) => return invalid(&message),
            Err(Malformed::Syntax(message)) => {
                return Outcome::Respond(failure("null", PARSE_ERROR, &message, None));
            }
        };
        let request = match request(members) {
            Ok(request) => request,
            Err(message) => return invalid(&message),
        };
        let Some(id) = request.id else {
            self.note(&format!(
                "ignored a notification, which cannot be answered: {:?}",
                request.method
            ));
            return Outcome::Silent;
        };
        if in_batch && ORDER_DEPENDENT.contains(&request.method.as_str()) {
            return Outcome::Respond(failure(
                id.get(),
                INVALID_REQUEST,
                &format!("{} is not allowed in a batch", request.method),
                None,
            ));
        }
        let params = request.params.as_deref().map(RawValue::get);
        match self.dispatcher.call(&request.method, params) {
            Ok(result) => {
                self.shutdown |= request.method == "shutdown";
                Outcome::Respond(success(id.get(), &result))
            }
            Err(error) => Outcome::Respond(failure(
                id.get(),
                error.code(),
                &error.message(),
                error.data().as_deref(),
            )),
        }
    }

    /// Handles one line of text. Returns the line to send, if any.
    fn message(&mut self, text: &str) -> Option<String> {
        match Top::parse(text) {
            Ok(Top::Array(entries)) => {
                if entries.is_empty() {
                    return Some(failure(
                        "null",
                        INVALID_REQUEST,
                        "a batch must not be empty",
                        None,
                    ));
                }
                let responses: Vec<String> = entries
                    .iter()
                    .filter_map(|entry| match self.entry(Top::parse(entry.get()), true) {
                        Outcome::Respond(line) => Some(line),
                        Outcome::Silent => None,
                    })
                    .collect();
                (!responses.is_empty()).then(|| format!("[{}]", responses.join(",")))
            }
            top => match self.entry(top, false) {
                Outcome::Respond(line) => Some(line),
                Outcome::Silent => None,
            },
        }
    }
}

/// Serves JSON-RPC requests from `input` until end of input or `shutdown`.
///
/// Each line of `input` is one message. Each response is one line on
/// `output`, flushed before the next line is read. Diagnostics go to `log`.
/// Returns an error only when `input` or `output` fails.
pub fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    log: impl Write,
    dispatcher: &mut Dispatcher,
    options: &ServerOptions,
) -> io::Result<()> {
    let mut session = Session {
        dispatcher,
        log,
        shutdown: false,
    };
    while !session.shutdown {
        let response = match read_line(&mut input, options.max_line_bytes)? {
            Line::End => break,
            Line::TooLong => Some(failure(
                "null",
                PARSE_ERROR,
                &format!("line is longer than {} bytes", options.max_line_bytes),
                None,
            )),
            Line::Bytes(bytes) if is_blank(&bytes) => None,
            Line::Bytes(bytes) => match std::str::from_utf8(&bytes) {
                Ok(text) => session.message(text),
                Err(_) => Some(failure("null", PARSE_ERROR, "line is not UTF-8", None)),
            },
        };
        if let Some(mut line) = response {
            line.push('\n');
            output.write_all(line.as_bytes())?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8], max: usize) -> Vec<Option<Vec<u8>>> {
        // A reader that hands out two bytes at a time, so lines cross buffers.
        let mut reader = io::BufReader::with_capacity(2, input);
        let mut seen = Vec::new();
        loop {
            match read_line(&mut reader, max).unwrap() {
                Line::End => return seen,
                Line::TooLong => seen.push(None),
                Line::Bytes(bytes) => seen.push(Some(bytes)),
            }
        }
    }

    /// Output that a reader can see only once it is flushed, and input that
    /// checks, before it hands over the second line, that the first line's
    /// response can be seen.
    #[test]
    fn each_response_is_flushed_before_the_next_line_is_read() {
        use std::{cell::RefCell, rc::Rc};

        struct Held {
            pending: Vec<u8>,
            shown: Rc<RefCell<Vec<u8>>>,
        }
        impl Write for Held {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.pending.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.shown.borrow_mut().append(&mut self.pending);
                Ok(())
            }
        }
        struct Checked {
            lines: Vec<u8>,
            position: usize,
            shown: Rc<RefCell<Vec<u8>>>,
        }
        impl io::Read for Checked {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let Some(byte) = self.lines.get(self.position) else {
                    return Ok(0);
                };
                let lines_given = self.lines[..self.position]
                    .iter()
                    .filter(|byte| **byte == b'\n')
                    .count();
                let answered = self
                    .shown
                    .borrow()
                    .iter()
                    .filter(|byte| **byte == b'\n')
                    .count();
                assert_eq!(
                    answered, lines_given,
                    "a line was read before the last answer was shown"
                );
                buffer[0] = *byte;
                self.position += 1;
                Ok(1)
            }
        }

        let shown = Rc::new(RefCell::new(Vec::new()));
        let input = Checked {
            lines: b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"none\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"none\"}\n".to_vec(),
            position: 0,
            shown: shown.clone(),
        };
        let output = Held {
            pending: Vec::new(),
            shown: shown.clone(),
        };
        serve(
            io::BufReader::with_capacity(1, input),
            output,
            io::sink(),
            &mut Dispatcher::new(Default::default()),
            &ServerOptions::default(),
        )
        .unwrap();
        assert_eq!(
            shown.borrow().iter().filter(|byte| **byte == b'\n').count(),
            2
        );
    }

    #[test]
    fn lines_end_at_newlines_and_the_last_needs_none() {
        assert_eq!(lines(b"", 8), Vec::<Option<Vec<u8>>>::new());
        assert_eq!(
            lines(b"ab\n\ncdefg", 8),
            [
                Some(b"ab".to_vec()),
                Some(Vec::new()),
                Some(b"cdefg".to_vec())
            ]
        );
        assert_eq!(lines(b"ab\n", 8), [Some(b"ab".to_vec())]);
        assert_eq!(lines(b"\n", 8), [Some(Vec::new())]);
    }

    #[test]
    fn a_line_at_the_limit_is_kept_and_one_byte_more_is_dropped_whole() {
        assert_eq!(lines(b"abcd\n", 4), [Some(b"abcd".to_vec())]);
        assert_eq!(lines(b"abcde\nxy\n", 4), [None, Some(b"xy".to_vec())]);
        assert_eq!(lines(b"abcdefghij", 4), [None]);
        assert_eq!(
            lines(b"a\nabcdefghij\nb", 4),
            [Some(b"a".to_vec()), None, Some(b"b".to_vec())]
        );
        assert_eq!(lines(b"x\n", 0), [None]);
        assert_eq!(lines(b"\n", 0), [Some(Vec::new())]);
    }
}
