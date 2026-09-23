//! Stateful, end-to-end scenarios for feature boundaries that unit tests do
//! not exercise: persistent cache reuse, inherited configuration, notebooks,
//! LSP sessions, filesystem watching, and the published composite action.

use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct ScenarioDir(PathBuf);

impl ScenarioDir {
    fn new(tag: &str) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("xray-scenario-{tag}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for ScenarioDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn xray_binary() -> &'static str {
    env!("CARGO_BIN_EXE_xray")
}

fn skip_process_scenarios_on_cross_runner() -> bool {
    if std::env::var_os("XRAY_SKIP_PROCESS_SCENARIOS").is_some() {
        eprintln!("skipping process scenario under a cross-compiled test runner");
        true
    } else {
        false
    }
}

fn run_xray(dir: &Path, args: &[&str]) -> Output {
    Command::new(xray_binary())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("xray process should start")
}

fn run_json(dir: &Path, args: &[&str]) -> Value {
    let output = run_xray(dir, args);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "xray failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("xray should emit JSON")
}

fn rule_ids(report: &Value) -> Vec<&str> {
    report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["rule_id"].as_str())
        .collect()
}

#[test]
fn cache_cold_warm_and_bypassed_runs_are_equivalent() {
    if skip_process_scenarios_on_cross_runner() {
        return;
    }
    let dir = ScenarioDir::new("cache-modes");
    dir.write(
        "analysis.py",
        "import cupy as cp\nimport numpy as np\nx = np.zeros((4, 4))\n",
    );
    dir.write("run.sh", "#!/bin/bash\n#SBATCH --gres=gpu:1\n");

    let args = ["--format", "json", "--job", "run.sh", "analysis.py"];
    let cold = run_json(dir.path(), &args);
    assert!(dir.path().join(".xray-cache").exists());
    let warm = run_json(dir.path(), &args);
    let bypassed = run_json(
        dir.path(),
        &[
            "--no-cache",
            "--format",
            "json",
            "--job",
            "run.sh",
            "analysis.py",
        ],
    );

    assert_eq!(cold, warm, "a warm cache must not change diagnostics");
    assert_eq!(cold, bypassed, "--no-cache must preserve semantics");
    assert_eq!(rule_ids(&cold), vec!["NP003"]);
    assert!(!rule_ids(&warm).contains(&"JOB004"));
}

#[test]
fn inherited_config_changes_invalidate_a_warm_cache() {
    if skip_process_scenarios_on_cross_runner() {
        return;
    }
    let dir = ScenarioDir::new("inherited-config-cache");
    let parent = dir.write("parent.toml", "[dask]\ncompute_call_threshold = 10\n");
    dir.write("child.toml", "extends = \"parent.toml\"\n");
    dir.write(
        "analysis.py",
        "import dask\na.compute()\nb.compute()\nc.compute()\nd.compute()\n",
    );
    let args = ["--config", "child.toml", "--format", "json", "analysis.py"];

    let inherited_ten = run_json(dir.path(), &args);
    assert!(!rule_ids(&inherited_ten).contains(&"DK003"));

    fs::write(parent, "[dask]\ncompute_call_threshold = 3\n").unwrap();
    let inherited_three = run_json(dir.path(), &args);
    assert!(
        rule_ids(&inherited_three).contains(&"DK003"),
        "a changed parent config must invalidate diagnostics cached under the old value"
    );
}

#[test]
fn notebook_cross_cell_bindings_are_stable_across_repeated_runs() {
    if skip_process_scenarios_on_cross_runner() {
        return;
    }
    let dir = ScenarioDir::new("notebook-cross-cell");
    let notebook = json!({
        "cells": [
            {
                "cell_type": "code",
                "execution_count": null,
                "metadata": {},
                "outputs": [],
                "source": ["import pandas as pd\n"]
            },
            {
                "cell_type": "code",
                "execution_count": null,
                "metadata": {},
                "outputs": [],
                "source": [
                    "df = pd.DataFrame({\"a\": [1]})\n",
                    "df.append({\"a\": 2})\n"
                ]
            }
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5
    });
    dir.write(
        "analysis.ipynb",
        serde_json::to_vec_pretty(&notebook).unwrap(),
    );

    let first = run_json(dir.path(), &["--format", "json", "analysis.ipynb"]);
    let second = run_json(dir.path(), &["--format", "json", "analysis.ipynb"]);
    assert_eq!(first, second);

    let pd002 = first["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["rule_id"] == "PD002")
        .expect("cross-cell pandas binding should produce PD002");
    assert_eq!(pd002["file"], "analysis.ipynb");
    assert_eq!(pd002["cell"], 2);
    assert_eq!(pd002["line"], 2);
}

fn send_lsp(writer: &mut impl Write, message: &Value) {
    let body = serde_json::to_vec(message).unwrap();
    write!(writer, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    writer.write_all(&body).unwrap();
    writer.flush().unwrap();
}

fn read_lsp(reader: &mut impl BufRead) -> Value {
    let mut length = None;
    loop {
        let mut line = String::new();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "unexpected LSP EOF"
        );
        if line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim().split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; length.expect("LSP response needs Content-Length")];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

struct ChildGuard {
    child: Child,
    finished: bool,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
fn lsp_open_close_shutdown_session_preserves_unicode_positions() {
    if skip_process_scenarios_on_cross_runner() {
        return;
    }
    let dir = ScenarioDir::new("lsp-session");
    let mut guard = ChildGuard {
        child: Command::new(xray_binary())
            .arg("lsp")
            .current_dir(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("LSP server should start"),
        finished: false,
    };
    let mut stdin = guard.child.stdin.take().unwrap();
    let mut stdout = BufReader::new(guard.child.stdout.take().unwrap());

    send_lsp(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );
    let initialized = read_lsp(&mut stdout);
    assert_eq!(initialized["id"], 1);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "xray");

    send_lsp(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": "file:///tmp/unicode.py",
                "text": "import numpy as np\n\"é\"; x = np.zeros((2, 2))\n"
            }}
        }),
    );
    let published = read_lsp(&mut stdout);
    let diagnostic = &published["params"]["diagnostics"][0];
    assert_eq!(diagnostic["code"], "NP003");
    assert_eq!(diagnostic["range"]["start"]["line"], 1);
    assert_eq!(diagnostic["range"]["start"]["character"], 9);

    send_lsp(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": {"textDocument": {"uri": "file:///tmp/unicode.py"}}
        }),
    );
    let cleared = read_lsp(&mut stdout);
    assert_eq!(cleared["params"]["diagnostics"], json!([]));

    send_lsp(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null}),
    );
    assert_eq!(read_lsp(&mut stdout)["id"], 2);
    send_lsp(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
    );
    drop(stdin);
    let status = guard.child.wait().unwrap();
    guard.finished = true;
    assert!(status.success());
}

#[derive(Clone, Copy)]
enum WatchStream {
    Stdout,
    Stderr,
}

fn forward_lines(
    reader: impl Read + Send + 'static,
    stream: WatchStream,
    tx: mpsc::Sender<(WatchStream, String)>,
    transcript: Arc<Mutex<Vec<String>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            transcript.lock().unwrap().push(line.clone());
            let _ = tx.send((stream, line));
        }
    })
}

fn wait_for_line(
    rx: &mpsc::Receiver<(WatchStream, String)>,
    timeout: Duration,
    predicate: impl Fn(WatchStream, &str) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        match rx.recv_timeout(remaining) {
            Ok((stream, line)) if predicate(stream, &line) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn watch_mode_rechecks_a_file_after_a_real_filesystem_event() {
    if skip_process_scenarios_on_cross_runner() {
        return;
    }
    let dir = ScenarioDir::new("watch-event");
    let source = dir.write("watched/analysis.py", "x = 1\n");
    let mut child = Command::new(xray_binary())
        .args(["--watch", "--no-cache", "--format", "json", "watched"])
        .current_dir(dir.path())
        .env("XRAY_WATCH_POLL", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("watch process should start");

    let (tx, rx) = mpsc::channel();
    let transcript = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = forward_lines(
        child.stdout.take().unwrap(),
        WatchStream::Stdout,
        tx.clone(),
        Arc::clone(&transcript),
    );
    let stderr_thread = forward_lines(
        child.stderr.take().unwrap(),
        WatchStream::Stderr,
        tx,
        Arc::clone(&transcript),
    );

    let ready = wait_for_line(&rx, Duration::from_secs(10), |stream, line| {
        matches!(stream, WatchStream::Stderr) && line.contains("xray: watching 1 root")
    });
    let mut diagnosed = false;
    if ready {
        for attempt in 0..5 {
            fs::write(
                &source,
                format!("import numpy as np\nx = np.zeros((4, 4))  # save {attempt}\n"),
            )
            .unwrap();
            if wait_for_line(&rx, Duration::from_secs(2), |stream, line| {
                matches!(stream, WatchStream::Stdout) && line.contains("\"rule_id\": \"NP003\"")
            }) {
                diagnosed = true;
                break;
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    let transcript = transcript.lock().unwrap().join("\n");
    assert!(ready, "watcher never became ready:\n{transcript}");
    assert!(diagnosed, "changed file was not re-linted:\n{transcript}");
}

#[cfg(unix)]
#[test]
fn published_action_preserves_globs_and_treats_inputs_as_data() {
    if skip_process_scenarios_on_cross_runner() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;

    let dir = ScenarioDir::new("github-action");
    dir.write("src/one.py", "x = 1\n");
    dir.write("nested/deep/two.py", "x = 2\n");
    let fake_xray = dir.write(
        "bin/xray",
        r#"#!/usr/bin/env bash
{
  echo BEGIN
  for arg in "$@"; do
    echo "ARG:$arg"
  done
  echo END
} >> "$XRAY_CAPTURE"
if [[ " $* " == *" --format json "* ]]; then
  printf '{"summary":{"total":0}}\n'
fi
"#,
    );
    let fake_python = dir.write(
        "bin/python3",
        "#!/usr/bin/env bash\ncat >/dev/null\nprintf '0\\n'\n",
    );
    for executable in [&fake_xray, &fake_python] {
        let mut permissions = fs::metadata(executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }

    let capture = dir.path().join("capture.txt");
    let output_file = dir.path().join("github-output.txt");
    let sentinel = dir.path().join("must-not-exist");
    let action_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(action_root.join("scripts/run-action.sh"))
        .current_dir(dir.path())
        .env(
            "PATH",
            format!("{}:{inherited_path}", dir.path().join("bin").display()),
        )
        .env("XRAY_CAPTURE", &capture)
        .env("GITHUB_WORKSPACE", dir.path())
        .env("GITHUB_OUTPUT", &output_file)
        .env("INPUT_FORMAT", "text")
        .env("INPUT_MIN_SEVERITY", "hint")
        .env("INPUT_FAIL_ON", "never")
        .env(
            "INPUT_PATHS",
            format!("**/*.py nested/**/*.py ;touch {}", sentinel.display()),
        )
        .env("INPUT_CONFIG", "config with spaces.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "action runner failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !sentinel.exists(),
        "an input string was evaluated as shell code"
    );

    let captured = fs::read_to_string(capture).unwrap();
    assert_eq!(captured.matches("BEGIN").count(), 2);
    assert_eq!(captured.matches("ARG:**/*.py").count(), 2);
    assert_eq!(captured.matches("ARG:nested/**/*.py").count(), 2);
    assert_eq!(captured.matches("ARG:config with spaces.toml").count(), 2);
    assert!(!captured.contains("ARG:src/one.py"));
    assert!(!captured.contains("ARG:nested/deep/two.py"));
    for invocation in captured.split("BEGIN").skip(1) {
        let args: Vec<&str> = invocation
            .lines()
            .filter_map(|line| line.strip_prefix("ARG:"))
            .collect();
        let separator = args.iter().position(|arg| *arg == "--").unwrap();
        let first_path = args.iter().position(|arg| *arg == "**/*.py").unwrap();
        assert!(
            separator < first_path,
            "path arguments need a `--` separator"
        );
    }
    assert_eq!(
        fs::read_to_string(output_file).unwrap().trim(),
        "issues-found=0"
    );

    let manifest = fs::read_to_string(action_root.join("action.yml")).unwrap();
    assert!(manifest.contains("bash \"$GITHUB_ACTION_PATH/scripts/run-action.sh\""));
}
