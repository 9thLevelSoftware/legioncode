//! Opaque-box `legioncode` server process & HTTP/CLI harness with Windows-safe Drop ordering.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread::{self, JoinHandle};

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use tempfile::TempDir;

use super::git_fixtures::init_git_repo;
use super::{TestResult, apply_isolated_env};

pub struct LegionServerProcess {
    child: Child,
    origin: String,
    addr: SocketAddr,
    pairing_url: Option<String>,
    stdout_drain: Option<JoinHandle<String>>,
    stderr_drain: Option<JoinHandle<String>>,
}

impl LegionServerProcess {
    pub fn spawn(home_dir: &Path, workspace_dir: &Path, static_dir: &Path) -> TestResult<Self> {
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

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.stderr.take().ok_or("missing child stderr")?;

        let stderr_drain = Some(thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = BufReader::new(stderr);
            let _ = reader.read_to_string(&mut buf);
            buf
        }));

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut origin: Option<String> = None;
        let mut pairing_url: Option<String> = None;

        loop {
            line.clear();
            let read_bytes = reader.read_line(&mut line)?;
            if read_bytes == 0 {
                break;
            }
            let trimmed = line.trim();
            if origin.is_none()
                && let Some(rest) = trimmed.strip_prefix("LegionCode server listening on ")
            {
                origin = Some(rest.trim().to_owned());
            }
            if pairing_url.is_none()
                && let Some(url) = trimmed.strip_prefix("pairingUrl:")
            {
                pairing_url = Some(url.trim().to_owned());
            }
            if origin.is_some() && pairing_url.is_some() {
                break;
            }
        }

        let Some(origin) = origin else {
            let _ = child.kill();
            let _ = child.wait();
            let err_tail = stderr_drain.and_then(|h| h.join().ok()).unwrap_or_default();
            return Err(format!(
                "legioncode serve exited before printing listening origin; stderr:\n{err_tail}"
            )
            .into());
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
            pairing_url,
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

    pub fn pairing_url(&self) -> Option<&str> {
        self.pairing_url.as_deref()
    }
}

impl Drop for LegionServerProcess {
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

/// Opaque-box test environment. `server` is declared before `_temp` so Drop kills and reaps
/// the child process (releasing Windows SQLite locks) before `TempDir` removes directories.
pub struct OpaqueBoxHarness {
    server: Option<LegionServerProcess>,
    http: reqwest::Client,
    home_dir: PathBuf,
    workspace_dir: PathBuf,
    static_dir: PathBuf,
    _temp: TempDir,
}

impl OpaqueBoxHarness {
    pub fn start(seed_git: bool) -> TestResult<Self> {
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

        let server = LegionServerProcess::spawn(&home_dir, &workspace_dir, &static_dir)?;
        Ok(Self {
            server: Some(server),
            http: reqwest::Client::new(),
            home_dir,
            workspace_dir,
            static_dir,
            _temp: temp,
        })
    }

    pub fn server(&self) -> TestResult<&LegionServerProcess> {
        self.server
            .as_ref()
            .ok_or_else(|| "server is not running".into())
    }

    pub fn origin(&self) -> TestResult<&str> {
        Ok(self.server()?.origin())
    }

    pub fn addr(&self) -> TestResult<SocketAddr> {
        Ok(self.server()?.addr())
    }

    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn restart_server(&mut self) -> TestResult<()> {
        drop(self.server.take());
        let next =
            LegionServerProcess::spawn(&self.home_dir, &self.workspace_dir, &self.static_dir)?;
        self.server = Some(next);
        Ok(())
    }

    pub fn run_cli(&self, args: &[&str]) -> TestResult<Output> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_legioncode"));
        cmd.args(args).current_dir(&self.workspace_dir);
        apply_isolated_env(&mut cmd, &self.home_dir);
        Ok(cmd.output()?)
    }

    pub async fn pair_client(&self, label: &str, scopes: Option<&[&str]>) -> TestResult<String> {
        let base_str = self.home_dir.to_string_lossy().into_owned();
        let mut args = vec!["pair", "--base-dir", &base_str, "--json", "--label", label];
        let joined_scopes = scopes.map(|s| s.join(","));
        if let Some(ref scope_val) = joined_scopes {
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
        let token = parsed["token"]
            .as_str()
            .ok_or("missing token in pair JSON output")?;

        let resp = self
            .http
            .post(format!("{}/api/auth/pair", self.origin()?))
            .json(&serde_json::json!({ "token": token, "clientLabel": label }))
            .send()
            .await?;
        if resp.status() != 200 {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("/api/auth/pair returned {status}: {text}").into());
        }
        let reply: Value = resp.json().await?;
        Ok(reply["accessToken"]
            .as_str()
            .ok_or("missing accessToken in /api/auth/pair response")?
            .to_owned())
    }

    pub async fn probe_environment(&self) -> TestResult<Value> {
        let resp = self
            .http
            .get(format!(
                "{}/.well-known/legioncode/environment",
                self.origin()?
            ))
            .send()
            .await?;
        if resp.status() != 200 {
            return Err(format!("environment probe returned {}", resp.status()).into());
        }
        Ok(resp.json().await?)
    }

    pub async fn post_mcp(
        &self,
        bearer: Option<&str>,
        extra_headers: &[(&str, &str)],
        body: &Value,
    ) -> TestResult<reqwest::Response> {
        let mut req = self
            .http
            .post(format!("{}/mcp", self.origin()?))
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json");
        if let Some(tok) = bearer {
            req = req.header(AUTHORIZATION, format!("Bearer {tok}"));
        }
        for (name, value) in extra_headers {
            req = req.header(*name, *value);
        }
        Ok(req.json(body).send().await?)
    }

    pub fn open_db_readonly(&self) -> TestResult<Connection> {
        let db_path = self.home_dir.join("userdata").join("statev2.sqlite");
        Ok(Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?)
    }
}
