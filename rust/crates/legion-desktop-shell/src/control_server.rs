//! Desktop App Activation Control Server supporting loopback TCP (for deterministic
//! cross-platform tests) plus Windows Named Pipes and Unix domain sockets (`legioncode app`).

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use crate::control_server_protocol::{
    ActivationHandler, DESKTOP_APP_ACTIVATION_PROTOCOL_VERSION, DesktopAppControlAddress,
    DesktopControlPlatform, DesktopControlRequest, DesktopControlResponse, DesktopControlStatus,
    MAX_CONTROL_REQUEST_BYTES, handle_activation_line, resolve_desktop_app_control_address,
};

const CONTROL_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors emitted when binding, accepting, or sending desktop activation requests.
#[derive(Debug, Error)]
pub enum ControlServerError {
    /// Binding, connecting, or writing over the control transport failed.
    #[error("desktop control transport I/O error: {0}")]
    Io(#[from] io::Error),
    /// Serializing or decoding activation JSON failed.
    #[error("desktop control JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Client timed out waiting for a newline-delimited response.
    #[error("desktop control request timed out")]
    Timeout,
    /// Server closed the stream before returning a response line.
    #[error("desktop control server closed connection without a response")]
    UnexpectedEof,
}

/// Running Desktop App Activation Control Server handle with graceful shutdown.
#[derive(Debug)]
pub struct DesktopControlServer {
    address: String,
    local_addr: Option<SocketAddr>,
    shutdown_tx: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    #[cfg(unix)]
    cleanup_socket: Option<std::path::PathBuf>,
}

impl DesktopControlServer {
    /// Returns the bound transport address (`127.0.0.1:<port>`, `\\.\pipe\...`, or `*.sock`).
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Returns the bound TCP [`SocketAddr`] when started with [`Self::bind_loopback`].
    pub const fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Binds a loopback TCP control server on `127.0.0.1:0` (cross-platform test & local IPC).
    pub async fn bind_loopback(handler: ActivationHandler) -> Result<Self, ControlServerError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let handler_ref = handler.clone();
                        tokio::spawn(async move {
                            let _ = serve_stream(stream, handler_ref).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            address: local_addr.to_string(),
            local_addr: Some(local_addr),
            shutdown_tx,
            task: Some(task),
            #[cfg(unix)]
            cleanup_socket: None,
        })
    }

    /// Binds a Windows Named Pipe server (`\\.\pipe\...`) for live `legioncode app` desktop activation.
    #[cfg(windows)]
    pub fn bind_named_pipe(
        pipe_name: impl Into<String>,
        handler: ActivationHandler,
    ) -> Result<Self, ControlServerError> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let pipe_name = pipe_name.into();
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)?;
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let loop_name = pipe_name.clone();

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    connected = server.connect() => {
                        if connected.is_err() {
                            break;
                        }
                        let client_pipe = server;
                        let handler_ref = handler.clone();
                        tokio::spawn(async move {
                            let _ = serve_stream(client_pipe, handler_ref).await;
                        });
                        match ServerOptions::new().create(&loop_name) {
                            Ok(next) => server = next,
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        Ok(Self {
            address: pipe_name,
            local_addr: None,
            shutdown_tx,
            task: Some(task),
        })
    }

    /// Binds a private `AF_UNIX` domain socket listener on POSIX hosts.
    #[cfg(unix)]
    pub fn bind_unix_socket(
        address: impl Into<String>,
        directory: Option<&str>,
        handler: ActivationHandler,
    ) -> Result<Self, ControlServerError> {
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::UnixListener;

        let address = address.into();
        if let Some(dir) = directory {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let socket_path = std::path::PathBuf::from(&address);
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }
        let listener = UnixListener::bind(&socket_path)?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let handler_ref = handler.clone();
                        tokio::spawn(async move {
                            let _ = serve_stream(stream, handler_ref).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            address,
            local_addr: None,
            shutdown_tx,
            task: Some(task),
            cleanup_socket: Some(socket_path),
        })
    }

    /// Binds the canonical OS control transport derived from `state_dir`.
    pub fn bind_for_state_dir(
        state_dir: &Path,
        handler: ActivationHandler,
    ) -> Result<Self, ControlServerError> {
        let temp_dir = std::env::temp_dir().to_string_lossy().into_owned();
        let resolved = resolve_desktop_app_control_address(
            &state_dir.to_string_lossy(),
            DesktopControlPlatform::current(),
            &temp_dir,
            None,
        );
        #[cfg(windows)]
        {
            Self::bind_named_pipe(resolved.address, handler)
        }
        #[cfg(unix)]
        {
            Self::bind_unix_socket(resolved.address, resolved.directory.as_deref(), handler)
        }
    }

    /// Stops accepting new connections and awaits the background accept task.
    pub async fn shutdown(mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        #[cfg(unix)]
        if let Some(path) = self.cleanup_socket.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for DesktopControlServer {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
        #[cfg(unix)]
        if let Some(path) = self.cleanup_socket.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Sends an activation request line to a loopback TCP [`DesktopControlServer`].
pub async fn send_loopback_request(
    addr: SocketAddr,
    request: &DesktopControlRequest,
) -> Result<DesktopControlResponse, ControlServerError> {
    let stream = TcpStream::connect(addr).await?;
    send_stream_request(stream, request).await
}

/// Sends an activation request line over a Windows Named Pipe (`\\.\pipe\...`).
#[cfg(windows)]
pub async fn send_named_pipe_request(
    pipe_name: &str,
    request: &DesktopControlRequest,
) -> Result<DesktopControlResponse, ControlServerError> {
    use tokio::net::windows::named_pipe::ClientOptions;

    const ERROR_PIPE_BUSY: i32 = 231;
    let mut attempts = 0_u32;
    let client = loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(pipe) => break pipe,
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < 20 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(err) => return Err(ControlServerError::Io(err)),
        }
    };
    send_stream_request(client, request).await
}

/// Sends an activation request line over a Unix domain socket.
#[cfg(unix)]
pub async fn send_unix_request(
    path: &str,
    request: &DesktopControlRequest,
) -> Result<DesktopControlResponse, ControlServerError> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    send_stream_request(stream, request).await
}

async fn serve_stream<S>(stream: S, handler: ActivationHandler) -> Result<(), ControlServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    let read_res = tokio::time::timeout(CONTROL_READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| ControlServerError::Timeout)?;
    let bytes = read_res?;
    if bytes == 0 {
        return Ok(());
    }

    let response = handle_activation_line(&line, &handler);
    let mut out = serde_json::to_string(&response)?;
    out.push('\n');
    write_half.write_all(out.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}

async fn send_stream_request<S>(
    mut stream: S,
    request: &DesktopControlRequest,
) -> Result<DesktopControlResponse, ControlServerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut payload = serde_json::to_string(request)?;
    payload.push('\n');
    stream.write_all(payload.as_bytes()).await?;
    stream.flush().await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let bytes = tokio::time::timeout(CONTROL_READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| ControlServerError::Timeout)??;
    if bytes == 0 {
        return Err(ControlServerError::UnexpectedEof);
    }
    let response: DesktopControlResponse = serde_json::from_str(line.trim_end())?;
    Ok(response)
}
