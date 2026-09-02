//! Minimal synchronous LSP server for `xray lsp`.
//!
//! Implements JSON-RPC 2.0 over stdin/stdout with the Language Server Protocol
//! subset needed for a diagnostic-only server:
//!
//!   • `initialize` / `initialized` — handshake
//!   • `textDocument/didOpen`  — lint the text sent by the client
//!   • `textDocument/didSave`  — re-lint the file from disk
//!   • `textDocument/didClose` — clear diagnostics for the closed file
//!   • `shutdown` / `exit`     — clean termination
//!
//! The server publishes `textDocument/publishDiagnostics` notifications
//! after every open/save event.  No async runtime is required.

use crate::{config::Config, parser, rules, runner};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufWriter, Write};

const SERVER_NAME: &str = "xray";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run the LSP server loop on stdin/stdout until `exit` is received.
/// Config is loaded once from the working directory at startup.
pub fn run_lsp() {
    let config = Config::from_dir(".").unwrap_or_default();

    let stdin = io::stdin();
    let stdout = io::stdout();

    let mut reader = io::BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());

    let mut shutdown_requested = false;

    while let Some(raw) = read_message(&mut reader) {
        let msg: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue, // malformed JSON — skip silently
        };

        let method = msg["method"].as_str().unwrap_or("");
        let id = msg.get("id").cloned(); // present on requests, absent on notifications
        let params = &msg["params"];

        match method {
            // ── Lifecycle ────────────────────────────────────────────────────
            "initialize" => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "capabilities": {
                            "textDocumentSync": {
                                "openClose": true,
                                // 0 = None (no incremental sync — we re-lint whole files)
                                "change": 0,
                                "save": { "includeText": false }
                            }
                        },
                        "serverInfo": {
                            "name": SERVER_NAME,
                            "version": SERVER_VERSION,
                        }
                    }
                });
                write_message(&mut writer, &response.to_string());
            }

            "initialized" => {} // notification — no response needed

            "shutdown" => {
                shutdown_requested = true;
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": null
                });
                write_message(&mut writer, &response.to_string());
            }

            "exit" => break,

            // ── Document sync ─────────────────────────────────────────────────
            "textDocument/didOpen" => {
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let text = params["textDocument"]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let diags = lint_text(text, &uri, &config);
                let notif = publish_diagnostics_notification(&uri, diags);
                write_message(&mut writer, &notif.to_string());
            }

            "textDocument/didSave" => {
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if let Some(path) = uri_to_path(&uri) {
                    let diags = lint_path(&path, &config);
                    let notif = publish_diagnostics_notification(&uri, diags);
                    write_message(&mut writer, &notif.to_string());
                }
            }

            "textDocument/didClose" => {
                // Clear diagnostics for files the user has closed.
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let notif = publish_diagnostics_notification(&uri, vec![]);
                write_message(&mut writer, &notif.to_string());
            }

            _ => {
                // Unknown method — send MethodNotFound for requests, ignore notifications
                if let Some(req_id) = id
                    && !shutdown_requested
                {
                    let err = json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "error": {
                            "code": -32601,
                            "message": format!("Method not found: {method}")
                        }
                    });
                    write_message(&mut writer, &err.to_string());
                }
            }
        }
    }
}

// ── JSON-RPC framing ──────────────────────────────────────────────────────────

/// Read one LSP message from `reader`.
/// Returns `None` on EOF or read error.
///
/// Format:
/// ```text
/// Content-Length: <bytes>\r\n
/// \r\n
/// <json body>
/// ```
pub fn read_message(reader: &mut impl BufRead) -> Option<String> {
    // Parse headers until we find Content-Length
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return None, // EOF
            Err(_) => return None,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break; // blank line separates headers from body
        }
        // Header names are case-insensitive; some clients send `content-length`.
        if let Some((name, val)) = trimmed.split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            content_length = val.trim().parse().ok();
        }
    }

    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// Write one LSP message to `writer` with the correct Content-Length header.
fn write_message(writer: &mut impl Write, json: &str) {
    let body = json.as_bytes();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).ok();
    writer.write_all(body).ok();
    writer.flush().ok();
}

// ── diagnostics ───────────────────────────────────────────────────────────────

fn lint_text(text: String, uri: &str, config: &Config) -> Vec<Value> {
    match parser::parse_source(text) {
        Ok(parsed) => {
            let mut diags = rules::run_all(&parsed, uri, config);
            runner::apply_config_filters(&mut diags, config);
            xray_diags_to_lsp(diags)
        }
        Err(_) => vec![],
    }
}

fn lint_path(path: &str, config: &Config) -> Vec<Value> {
    match parser::parse_file(path) {
        Ok(parsed) => {
            let mut diags = rules::run_all(&parsed, path, config);
            runner::apply_config_filters(&mut diags, config);
            xray_diags_to_lsp(diags)
        }
        Err(_) => vec![],
    }
}

fn xray_diags_to_lsp(diags: Vec<crate::diagnostic::Diagnostic>) -> Vec<Value> {
    use crate::diagnostic::Severity;
    diags
        .into_iter()
        .map(|d| {
            // LSP lines are 0-based; xray lines are 1-based
            let line = d.line.saturating_sub(1) as u32;
            let col = d.column.saturating_sub(1) as u32;

            let severity_code: u32 = match d.severity {
                Severity::Error => 1,
                Severity::Warning => 2,
                Severity::Hint => 4, // LSP: 3 = Information, 4 = Hint
            };

            let mut lsp_diag = json!({
                "range": {
                    "start": { "line": line, "character": col },
                    // End at EOL — we don't track token end positions
                    "end":   { "line": line, "character": 999 }
                },
                "severity": severity_code,
                "code": d.rule_id,
                "source": "xray",
                "message": d.message,
            });

            // Attach the docs URL as a related information link if present
            if let Some(url) = d.url {
                lsp_diag["codeDescription"] = json!({ "href": url });
            }

            lsp_diag
        })
        .collect()
}

fn publish_diagnostics_notification(uri: &str, diagnostics: Vec<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "diagnostics": diagnostics,
        }
    })
}

// ── URI helpers ───────────────────────────────────────────────────────────────

/// Convert a `file://` URI to a filesystem path.
/// Returns `None` for non-file URIs (e.g. `untitled:`, `git://`).
pub fn uri_to_path(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://")?;
    // On Windows, strip the leading slash before the drive letter
    #[cfg(windows)]
    let path = path.trim_start_matches('/');
    // Decode %20-style percent-encoding for common characters
    Some(percent_decode(path))
}

/// Decode percent-escapes into bytes, then interpret the result as UTF-8.
///
/// Decoding each escape straight into a `char` treated the byte as Latin-1, so
/// a path containing `%C3%A9` came back as `Ã©` rather than `é` and the file
/// could not be opened.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            // A malformed escape is kept verbatim rather than dropped.
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_message(body: &str) -> Vec<u8> {
        let mut msg = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        msg.extend_from_slice(body.as_bytes());
        msg
    }

    #[test]
    fn read_message_parses_content_length() {
        let body = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
        let raw = make_message(body);
        let mut reader = io::BufReader::new(Cursor::new(raw));
        let result = read_message(&mut reader).expect("should parse");
        assert_eq!(result, body);
    }

    #[test]
    fn read_message_returns_none_on_eof() {
        let mut reader = io::BufReader::new(Cursor::new(b"" as &[u8]));
        assert!(read_message(&mut reader).is_none());
    }

    #[test]
    fn read_message_handles_extra_headers() {
        let body = r#"{"method":"exit"}"#;
        let raw = format!(
            "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut reader = io::BufReader::new(Cursor::new(raw.into_bytes()));
        let result = read_message(&mut reader).expect("should parse");
        assert_eq!(result, body);
    }

    #[test]
    fn uri_to_path_strips_file_scheme() {
        assert_eq!(
            uri_to_path("file:///home/user/project/analysis.py"),
            Some("/home/user/project/analysis.py".to_string())
        );
    }

    #[test]
    fn uri_to_path_returns_none_for_non_file() {
        assert!(uri_to_path("untitled:Untitled-1").is_none());
        assert!(uri_to_path("git://github.com/foo/bar").is_none());
    }

    #[test]
    fn uri_to_path_decodes_percent_encoding() {
        assert_eq!(
            uri_to_path("file:///home/user/my%20project/file.py"),
            Some("/home/user/my project/file.py".to_string())
        );
    }

    #[test]
    fn xray_diag_severity_maps_to_lsp_codes() {
        use crate::diagnostic::{Diagnostic, Severity};
        let diags = vec![
            Diagnostic::new("XR001", Severity::Error, "f.py", 1, 1, "err"),
            Diagnostic::new("XR002", Severity::Warning, "f.py", 2, 1, "warn"),
            Diagnostic::new("XR003", Severity::Hint, "f.py", 3, 1, "hint"),
        ];
        let lsp = xray_diags_to_lsp(diags);
        assert_eq!(lsp[0]["severity"], 1); // Error
        assert_eq!(lsp[1]["severity"], 2); // Warning
        assert_eq!(lsp[2]["severity"], 4); // Hint
    }

    #[test]
    fn xray_diag_line_converted_to_zero_based() {
        use crate::diagnostic::{Diagnostic, Severity};
        let d = Diagnostic::new("XR001", Severity::Warning, "f.py", 10, 5, "msg");
        let lsp = xray_diags_to_lsp(vec![d]);
        assert_eq!(lsp[0]["range"]["start"]["line"], 9); // 10 - 1
        assert_eq!(lsp[0]["range"]["start"]["character"], 4); // 5 - 1
    }
}
