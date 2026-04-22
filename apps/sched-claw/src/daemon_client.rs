use crate::app_config::DaemonClientConfig;
use crate::daemon_protocol::{
    DEFAULT_LOG_TAIL_LINES, DaemonLogsSnapshot, DaemonStatusSnapshot, SchedExtDaemonRequest,
    SchedExtDaemonResponse,
};
use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::{Duration, timeout};

#[derive(Clone, Debug)]
pub struct SchedExtDaemonClient {
    config: DaemonClientConfig,
}

impl SchedExtDaemonClient {
    #[must_use]
    pub fn new(config: DaemonClientConfig) -> Self {
        Self { config }
    }

    pub async fn send(&self, request: &SchedExtDaemonRequest) -> Result<SchedExtDaemonResponse> {
        let timeout_window = Duration::from_millis(self.config.request_timeout_ms);
        let stream = timeout(
            timeout_window,
            UnixStream::connect(&self.config.socket_path),
        )
        .await
        .with_context(|| {
            format!(
                "timed out connecting to daemon socket {}",
                self.config.socket_path.display()
            )
        })??;
        let (read_half, mut write_half) = stream.into_split();
        let payload = serde_json::to_vec(request)?;
        timeout(timeout_window, async {
            write_half.write_all(&payload).await?;
            write_half.write_all(b"\n").await?;
            write_half.shutdown().await
        })
        .await
        .context("timed out sending daemon request")??;

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        timeout(timeout_window, reader.read_line(&mut line))
            .await
            .context("timed out waiting for daemon response")??;
        if line.trim().is_empty() {
            bail!("daemon returned an empty response");
        }
        Ok(serde_json::from_str(line.trim())?)
    }

    pub async fn status(&self) -> Result<DaemonStatusSnapshot> {
        match self.send(&SchedExtDaemonRequest::Status {}).await? {
            SchedExtDaemonResponse::Status { snapshot }
            | SchedExtDaemonResponse::Ack { snapshot, .. } => Ok(snapshot),
            SchedExtDaemonResponse::Error { message } => bail!(message),
            other => bail!("daemon returned unexpected response for status: {other:?}"),
        }
    }

    pub async fn logs(&self, tail_lines: Option<usize>) -> Result<DaemonLogsSnapshot> {
        match self
            .send(&SchedExtDaemonRequest::Logs {
                tail_lines: Some(tail_lines.unwrap_or(DEFAULT_LOG_TAIL_LINES)),
            })
            .await?
        {
            SchedExtDaemonResponse::Logs { snapshot } => Ok(snapshot),
            SchedExtDaemonResponse::Error { message } => bail!(message),
            other => bail!("daemon returned unexpected response for logs: {other:?}"),
        }
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.config.socket_path
    }
}

pub fn render_response_text(response: &SchedExtDaemonResponse) -> String {
    match response {
        SchedExtDaemonResponse::Status { snapshot } => render_status(snapshot),
        SchedExtDaemonResponse::Logs { snapshot } => render_logs(snapshot),
        SchedExtDaemonResponse::Ack { message, snapshot } => {
            format!("{message}\n\n{}", render_status(snapshot))
        }
        SchedExtDaemonResponse::Error { message } => format!("daemon error: {message}"),
    }
}

fn render_status(snapshot: &DaemonStatusSnapshot) -> String {
    let mut lines = vec![
        format!("daemon_pid: {}", snapshot.daemon_pid),
        format!("workspace_root: {}", snapshot.workspace_root),
        format!("socket_path: {}", snapshot.socket_path),
    ];
    if !snapshot.allowed_roots.is_empty() {
        lines.push(format!(
            "allowed_roots: {}",
            snapshot.allowed_roots.join(", ")
        ));
    }
    match (&snapshot.active, &snapshot.last_exit) {
        (Some(active), _) => {
            lines.push("active: yes".to_string());
            lines.push(format!("active_label: {}", active.label));
            lines.push(format!("active_pid: {}", active.pid));
            lines.push(format!("active_cwd: {}", active.cwd));
            lines.push(format!("active_argv: {}", active.argv.join(" ")));
            lines.push(format!("active_log_lines: {}", active.log_line_count));
        }
        (None, Some(last_exit)) => {
            lines.push("active: no".to_string());
            lines.push(format!("last_label: {}", last_exit.label));
            lines.push(format!(
                "last_exit: code={:?} signal={:?}",
                last_exit.exit_code, last_exit.signal
            ));
            lines.push(format!("last_cwd: {}", last_exit.cwd));
            lines.push(format!("last_argv: {}", last_exit.argv.join(" ")));
            lines.push(format!("last_log_lines: {}", last_exit.log_line_count));
        }
        (None, None) => {
            lines.push("active: no".to_string());
            lines.push("last_exit: none".to_string());
        }
    }
    lines.join("\n")
}

fn render_logs(snapshot: &DaemonLogsSnapshot) -> String {
    let mut lines = vec![format!(
        "active_label: {}",
        snapshot.active_label.as_deref().unwrap_or("<none>")
    )];
    lines.push(format!("truncated: {}", snapshot.truncated));
    if snapshot.lines.is_empty() {
        lines.push("logs: <empty>".to_string());
        return lines.join("\n");
    }
    lines.push("logs:".to_string());
    for entry in &snapshot.lines {
        lines.push(format!(
            "[{}][{}] {}",
            entry.emitted_at_unix_ms, entry.source, entry.line
        ));
    }
    lines.join("\n")
}
