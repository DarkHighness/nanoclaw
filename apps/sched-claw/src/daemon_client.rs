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
