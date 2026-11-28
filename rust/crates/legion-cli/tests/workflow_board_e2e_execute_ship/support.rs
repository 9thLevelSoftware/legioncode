//! Core crashable server harness, auth/pairing, SQLite, and git helpers for E2E-M3.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::workflow_board_e2e::git_fixtures::init_git_repo;
use crate::workflow_board_e2e::ws_rpc::OpaqueWsClient;
use crate::workflow_board_e2e::{TestResult, apply_isolated_env};

static NEXT_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn next_id(prefix: &str) -> String {
    let n = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{n}")
}

pub struct SpawnedServer {
    child: Child,
    origin: String,
    addr: SocketAddr,
    stdout_drain: Option<JoinHandle<String>>,
    stderr_drain: Option<JoinHandle<String>>,
}

impl SpawnedServer {
    fn spawn(
        home_dir: &Path,
        workspace_dir: &Path,
        static_dir: &Path,
        extra_env: &[(String, String)],
    ) -> TestResult<Self> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_legioncode"));
        cmd.args([
            "serve",
            "--port",
            "0",
            "--host",
            "127.0.0.1",
            "--base-dir",
            &home_dir.to_string_lossy(),
            "--no-browser",
            "--static-dir",
            &static_dir.to_string_lossy(),
        ])
        .current_dir(workspace_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        apply_isolated_env(&mut cmd, home_dir);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.stderr.take().ok_or("missing child stderr")?;
        let stderr_drain = Some(thread::spawn(move || {
            let mut buf = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut buf);
            buf
        }));

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut origin: Option<String> = None;
        let mut has_pair = false;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let trimmed = line.trim();
            if origin.is_none()
                && let Some(rest) = trimmed.strip_prefix("LegionCode server listening on ")
            {
                origin = Some(rest.trim().to_owned());
            }
            if trimmed.starts_with("pairingUrl:") {
                has_pair = true;
            }
            if origin.is_some() && has_pair {
                break;
            }
        }
        let Some(origin) = origin else {
            let _ = child.kill();
            let _ = child.wait();
            let err_tail = stderr_drain.and_then(|h| h.join().ok()).unwrap_or_default();
            return Err(format!("legioncode serve exited early; stderr:\n{err_tail}").into());
        };
        let addr_str = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .ok_or_else(|| format!("unexpected origin format: {origin}"))?;
        let addr: SocketAddr = addr_str.parse()?;
        let stdout_drain = Some(thread::spawn(move || {
            let mut rest = String::new();
            let _ = reader.read_to_string(&mut rest);
            rest
        }));
        Ok(Self {
            child,
            origin,
            addr,
            stdout_drain,
            stderr_drain,
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for SpawnedServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stdout_drain.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_drain.take() {
            let _ = handle.join();
        }
    }
}

/// Crash-capable opaque-box harness (`server` declared before `temp_dir` for Windows lock release).
pub struct CrashableHarness {
    server: Option<SpawnedServer>,
    http: reqwest::Client,
    home_dir: PathBuf,
    workspace_dir: PathBuf,
    static_dir: PathBuf,
    extra_env: Vec<(String, String)>,
    temp_dir: TempDir,
}

impl CrashableHarness {
    pub fn start(seed_git: bool) -> TestResult<Self> {
        Self::start_with_env(seed_git, &[])
    }

    pub fn start_with_env(seed_git: bool, extra_env: &[(&str, &str)]) -> TestResult<Self> {
        let temp = tempfile::tempdir()?;
        let home_dir = temp.path().join("home");
        let workspace_dir = temp.path().join("workspace");
        let static_dir = temp.path().join("static");
        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&workspace_dir)?;
        fs::create_dir_all(&static_dir)?;
        if seed_git {
            init_git_repo(&workspace_dir)?;
        }
        let owned_env: Vec<(String, String)> = extra_env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let server = SpawnedServer::spawn(&home_dir, &workspace_dir, &static_dir, &owned_env)?;
        Ok(Self {
            server: Some(server),
            http: reqwest::Client::new(),
            home_dir,
            workspace_dir,
            static_dir,
            extra_env: owned_env,
            temp_dir: temp,
        })
    }

    pub fn stop_server(&mut self) {
        drop(self.server.take());
    }

    pub fn restart_server(&mut self) -> TestResult<()> {
        self.stop_server();
        let next = SpawnedServer::spawn(
            &self.home_dir,
            &self.workspace_dir,
            &self.static_dir,
            &self.extra_env,
        )?;
        self.server = Some(next);
        Ok(())
    }

    pub fn server(&self) -> TestResult<&SpawnedServer> {
        self.server
            .as_ref()
            .ok_or_else(|| "server is stopped".into())
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn temp_root(&self) -> &Path {
        self.temp_dir.path()
    }

    pub fn open_db_readonly(&self) -> TestResult<Connection> {
        Ok(Connection::open_with_flags(
            self.home_dir.join("userdata").join("statev2.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?)
    }

    pub fn open_db_rw_while_stopped(&self) -> TestResult<Connection> {
        if self.server.is_some() {
            return Err("stop_server() must be called before opening RW SQLite".into());
        }
        Ok(Connection::open(
            self.home_dir.join("userdata").join("statev2.sqlite"),
        )?)
    }

    pub fn run_cli(&self, args: &[&str]) -> TestResult<Output> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_legioncode"));
        cmd.args(args).current_dir(&self.workspace_dir);
        apply_isolated_env(&mut cmd, &self.home_dir);
        Ok(cmd.output()?)
    }

    pub async fn probe_environment(&self) -> TestResult<Value> {
        let origin = self.server()?.origin();
        let resp = self
            .http
            .get(format!("{origin}/.well-known/legioncode/environment"))
            .send()
            .await?;
        if resp.status() != 200 {
            return Err(format!("environment probe returned {}", resp.status()).into());
        }
        Ok(resp.json().await?)
    }

    pub async fn pair_client(&self, label: &str, scopes: Option<&[&str]>) -> TestResult<String> {
        let base_str = self.home_dir.to_string_lossy().into_owned();
        let mut args = vec!["pair", "--base-dir", &base_str, "--json", "--label", label];
        let joined = scopes.map(|s| s.join(","));
        if let Some(ref scope_val) = joined {
            args.push("--scope");
            args.push(scope_val);
        }
        let out = self.run_cli(&args)?;
        if !out.status.success() {
            return Err(format!(
                "legioncode pair failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )
            .into());
        }
        let parsed: Value = serde_json::from_slice(&out.stdout)?;
        let credential = parsed["token"]
            .as_str()
            .ok_or("missing token in pair output")?;
        self.exchange_pairing_token(credential, label).await
    }

    pub async fn exchange_pairing_token(&self, token: &str, label: &str) -> TestResult<String> {
        let origin = self.server()?.origin();
        let resp = self
            .http
            .post(format!("{origin}/api/auth/pair"))
            .json(&json!({ "token": token, "clientLabel": label }))
            .send()
            .await?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("/api/auth/pair returned {status}: {text}").into());
        }
        let body: Value = resp.json().await?;
        Ok(body["accessToken"]
            .as_str()
            .ok_or("missing accessToken in /api/auth/pair")?
            .to_owned())
    }

    /// Mints a real thread-scoped (`subject = "thread:<id>"`) bearer token via `auth_pairing_links`.
    pub async fn mint_agent_thread_token(&mut self, thread_id: &str) -> TestResult<String> {
        let credential = next_id("AGNT");
        let link_id = next_id("link-agent");
        let subject = format!("thread:{thread_id}");
        let scopes_json = json!(["orchestration:read", "orchestration:operate"]).to_string();
        self.stop_server();
        {
            let conn = self.open_db_rw_while_stopped()?;
            conn.execute(
                "INSERT INTO auth_pairing_links (
                   id, credential, method, scopes, subject, label, created_at, expires_at,
                   consumed_at, revoked_at, proof_key_thumbprint
                 ) VALUES (?1, ?2, 'one-time-token', ?3, ?4, 'agent-e2e', '2020-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL, NULL, NULL)",
                params![link_id, credential, scopes_json, subject],
            )?;
        }
        self.restart_server()?;
        self.exchange_pairing_token(&credential, "agent-thread-client")
            .await
    }

    pub async fn connect_ws(&self, token: &str) -> TestResult<OpaqueWsClient> {
        let auth_hdr = format!("Bearer {token}");
        OpaqueWsClient::connect(
            self.server()?.addr(),
            "/ws?orchestrationProtocol=2&frameParts=1",
            &[("Authorization", &auth_hdr)],
        )
        .await
    }

    pub async fn post_mcp(
        &self,
        bearer: Option<&str>,
        extra_headers: &[(&str, &str)],
        body: &Value,
    ) -> TestResult<reqwest::Response> {
        let origin = self.server()?.origin();
        let mut req = self
            .http
            .post(format!("{origin}/mcp"))
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json");
        if let Some(tok) = bearer {
            req = req.header("authorization", format!("Bearer {tok}"));
        }
        for (k, v) in extra_headers {
            req = req.header(*k, *v);
        }
        Ok(req.json(body).send().await?)
    }
}

pub fn run_git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn init_bare_remote(dir: &Path) -> TestResult<()> {
    fs::create_dir_all(dir)?;
    run_git(dir, &["init", "--bare"])?;
    Ok(())
}

pub fn assert_zero_threads(h: &CrashableHarness) -> TestResult<()> {
    let db = h.open_db_readonly()?;
    let count: i64 = db.query_row(
        "SELECT COUNT(*) FROM orchestration_v2_projection_threads",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 0);
    Ok(())
}
