use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{OnceLock, mpsc},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use super::decoder::{DecodedAudio, RtpFrameDecoder};

const FRAME_MAGIC: [u8; 2] = *b"WP";
const FRAME_VERSION: u8 = 1;
const FRAME_HEADER_LEN: usize = 8;
const MAX_CONTROL_FRAME_SIZE: u32 = 64 * 1024;
const MAX_RTP_FRAME_SIZE: u32 = 256 * 1024;
const WORKER_ENV: &str = "LIVE777_WHEPPROBE_BIN";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum FrameKind {
    Configure = 1,
    Rtp = 2,
    Response = 3,
}

impl TryFrom<u8> for FrameKind {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Configure),
            2 => Ok(Self::Rtp),
            3 => Ok(Self::Response),
            _ => bail!("unknown decode-worker frame kind {value}"),
        }
    }
}

impl FrameKind {
    fn size_limit(self) -> u32 {
        match self {
            Self::Rtp => MAX_RTP_FRAME_SIZE,
            Self::Configure | Self::Response => MAX_CONTROL_FRAME_SIZE,
        }
    }
}

#[derive(Debug)]
struct Frame {
    kind: FrameKind,
    payload: Vec<u8>,
}

async fn read_frame<R>(reader: &mut R) -> Result<Option<Frame>>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; FRAME_HEADER_LEN];
    if reader.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    reader
        .read_exact(&mut header[1..])
        .await
        .context("truncated decode-worker frame header")?;

    if header[..2] != FRAME_MAGIC {
        bail!("invalid decode-worker frame magic");
    }
    if header[2] != FRAME_VERSION {
        bail!(
            "unsupported decode-worker protocol version {} (expected {FRAME_VERSION})",
            header[2]
        );
    }
    let kind = FrameKind::try_from(header[3])?;
    let len = u32::from_be_bytes(header[4..].try_into().expect("four-byte frame length"));
    let limit = kind.size_limit();
    if len > limit {
        bail!("decode-worker {kind:?} frame is {len} bytes; limit is {limit}");
    }

    let mut payload = vec![0; len as usize];
    reader
        .read_exact(&mut payload)
        .await
        .context("truncated decode-worker frame payload")?;
    Ok(Some(Frame { kind, payload }))
}

fn encode_frame(kind: FrameKind, payload: &[u8]) -> Result<Vec<u8>> {
    let len = u32::try_from(payload.len()).context("decode-worker frame length exceeds u32")?;
    let limit = kind.size_limit();
    if len > limit {
        bail!("decode-worker {kind:?} frame is {len} bytes; limit is {limit}");
    }
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    frame.extend_from_slice(&FRAME_MAGIC);
    frame.push(FRAME_VERSION);
    frame.push(kind as u8);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

async fn write_frame<W>(writer: &mut W, kind: FrameKind, payload: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(&encode_frame(kind, payload)?).await?;
    writer.flush().await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodeWorkerConfig {
    pub mime_type: String,
    pub sprop_params: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoDecodeResult {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "media", content = "result", rename_all = "snake_case")]
pub enum DecodeSummary {
    Video(VideoDecodeResult),
    Audio(DecodedAudio),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WorkerResponse {
    Complete { result: DecodeSummary },
    Error { error: String },
}

impl WorkerResponse {
    fn into_result(self) -> Result<DecodeSummary> {
        match self {
            Self::Complete { result } => Ok(result),
            Self::Error { error } => Err(anyhow!(error)),
        }
    }
}

/// A decode child with no supervisor task: dropping this value synchronously
/// kills and reaps the child, including when an owning async future is dropped.
pub struct DecodeWorker {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
}

impl DecodeWorker {
    pub async fn start(
        config: DecodeWorkerConfig,
        ct: &CancellationToken,
        hard_deadline: Instant,
    ) -> Result<Self> {
        let path = resolve_worker_binary()?;
        Self::start_at(path, config, ct, hard_deadline).await
    }

    async fn start_at(
        path: PathBuf,
        config: DecodeWorkerConfig,
        ct: &CancellationToken,
        hard_deadline: Instant,
    ) -> Result<Self> {
        let mut command = Command::new(&path);
        command
            .arg("--decode-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Protocol output is stdout only; diagnostics use the inherited
            // stderr, so no undrained pipe can block the child either.
            .stderr(Stdio::inherit());
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn decode worker {}", path.display()))?;
        let stdin = child.stdin.take().expect("decode worker stdin was piped");
        let stdout = child.stdout.take().expect("decode worker stdout was piped");
        let mut worker = Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(stdout),
        };
        let config = serde_json::to_vec(&config).context("failed to encode worker config")?;
        worker
            .write_protocol_frame(FrameKind::Configure, &config, ct, hard_deadline)
            .await?;
        Ok(worker)
    }

    pub async fn send_rtp(
        &mut self,
        packet: &[u8],
        ct: &CancellationToken,
        hard_deadline: Instant,
    ) -> Result<()> {
        self.write_protocol_frame(FrameKind::Rtp, packet, ct, hard_deadline)
            .await
    }

    async fn write_protocol_frame(
        &mut self,
        kind: FrameKind,
        payload: &[u8],
        ct: &CancellationToken,
        hard_deadline: Instant,
    ) -> Result<()> {
        enum SendOutcome {
            Sent,
            Failed(anyhow::Error),
            Exited(String),
        }

        let frame = match encode_frame(kind, payload) {
            Ok(frame) => frame,
            Err(error) => {
                self.terminate().await;
                return Err(error);
            }
        };
        let outcome = {
            let stdin = self.stdin.as_mut().expect("open worker has stdin");
            let child = self.child.as_mut().expect("open worker has a child");
            tokio::select! {
                _ = ct.cancelled() => SendOutcome::Failed(anyhow!("decode worker send was cancelled")),
                _ = tokio::time::sleep_until(hard_deadline) => {
                    SendOutcome::Failed(anyhow!("decode worker send timed out"))
                }
                status = child.wait() => {
                    match status {
                        Ok(status) => SendOutcome::Exited(status.to_string()),
                        Err(error) => SendOutcome::Failed(anyhow!("failed to wait for decode worker during send: {error}")),
                    }
                }
                result = stdin.write_all(&frame) => {
                    match result.context("failed to write decode-worker frame") {
                        Ok(()) => SendOutcome::Sent,
                        Err(error) => SendOutcome::Failed(error),
                    }
                }
            }
        };
        let error = match outcome {
            SendOutcome::Sent => return Ok(()),
            SendOutcome::Failed(error) => error,
            SendOutcome::Exited(status) => {
                self.stdin.take();
                match self.read_response(ct, hard_deadline).await {
                    Ok(WorkerResponse::Error { error }) => anyhow!(error),
                    Ok(WorkerResponse::Complete { .. }) => anyhow!(
                        "decode worker exited during send ({status}) after returning a result"
                    ),
                    Err(error) => {
                        anyhow!("decode worker exited during send: {status}; {error:#}")
                    }
                }
            }
        };
        self.terminate().await;
        Err(error)
    }

    async fn read_response(
        &mut self,
        ct: &CancellationToken,
        hard_deadline: Instant,
    ) -> Result<WorkerResponse> {
        let stdout = self.stdout.as_mut().expect("open worker has stdout");
        tokio::select! {
            _ = ct.cancelled() => Err(anyhow!("decode worker result collection was cancelled")),
            _ = tokio::time::sleep_until(hard_deadline) => {
                Err(anyhow!("decode worker result collection timed out"))
            }
            frame = read_frame(stdout) => {
                match frame {
                    Ok(Some(Frame { kind: FrameKind::Response, payload })) => {
                        serde_json::from_slice::<WorkerResponse>(&payload)
                            .context("invalid decode-worker response")
                    }
                    Ok(Some(frame)) => Err(anyhow!(
                        "unexpected decode-worker output frame {:?}",
                        frame.kind
                    )),
                    Ok(None) => Err(anyhow!("decode worker closed stdout without a result")),
                    Err(error) => Err(error),
                }
            }
        }
    }

    /// Close stdin so the worker flushes its decoder, then collect its one
    /// structured response and exit status before releasing ownership.
    pub async fn finish(
        mut self,
        ct: &CancellationToken,
        hard_deadline: Instant,
    ) -> Result<DecodeSummary> {
        self.stdin.take();
        let response = match self.read_response(ct, hard_deadline).await {
            Ok(response) => response,
            Err(error) => {
                self.terminate().await;
                return Err(error);
            }
        };

        let status = {
            let child = self.child.as_mut().expect("open worker has a child");
            tokio::select! {
                _ = ct.cancelled() => Err(anyhow!("decode worker exit wait was cancelled")),
                _ = tokio::time::sleep_until(hard_deadline) => {
                    Err(anyhow!("decode worker exit wait timed out"))
                }
                status = child.wait() => status.context("failed to reap decode worker"),
            }
        };
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                self.terminate().await;
                return Err(error);
            }
        };
        if !status.success() {
            bail!("decode worker exited unsuccessfully: {status}");
        }
        response.into_result()
    }

    async fn terminate(&mut self) {
        self.stdin.take();
        self.stdout.take();
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) | Err(_) => {
                    let _ = child.start_kill();
                }
            }
            let _ = child.wait().await;
        }
    }
}

/// A single long-lived thread reaps dropped workers. `Drop` runs on an async
/// runtime thread, so it must not poll the kill: hand the child over and let
/// this thread wait for it.
static REAPER: OnceLock<Option<mpsc::Sender<Child>>> = OnceLock::new();

fn reaper() -> Option<&'static mpsc::Sender<Child>> {
    REAPER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Child>();
            std::thread::Builder::new()
                .name("decode-worker-reaper".to_string())
                .spawn(move || {
                    while let Ok(mut child) = rx.recv() {
                        let _ = child.start_kill();
                        while let Ok(None) = child.try_wait() {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                    }
                })
                .ok()
                .map(|_| tx)
        })
        .as_ref()
}

/// Fallback for the unlikely case where the reaper thread could not start.
fn reap_blocking(mut child: Child) {
    while let Ok(None) = child.try_wait() {
        std::thread::sleep(Duration::from_millis(1));
    }
}

impl Drop for DecodeWorker {
    fn drop(&mut self) {
        self.stdin.take();
        self.stdout.take();
        let Some(mut child) = self.child.take() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) | Err(_) => {
                let _ = child.start_kill();
            }
        }
        match reaper() {
            Some(tx) => {
                if let Err(error) = tx.send(child) {
                    reap_blocking(error.0);
                }
            }
            None => reap_blocking(child),
        }
    }
}

fn worker_binary_name() -> String {
    format!("whepprobe{}", std::env::consts::EXE_SUFFIX)
}

fn resolve_worker_binary() -> Result<PathBuf> {
    let current_exe = std::env::current_exe().context("failed to locate current executable")?;
    resolve_worker_binary_from(std::env::var_os(WORKER_ENV), &current_exe)
}

fn resolve_worker_binary_from(
    override_path: Option<OsString>,
    current_exe: &Path,
) -> Result<PathBuf> {
    if let Some(path) = override_path.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let sibling = current_exe.with_file_name(worker_binary_name());
    if sibling.is_file() {
        return Ok(sibling);
    }

    let current_dir = current_exe
        .parent()
        .context("current executable has no parent directory")?;
    if current_dir.file_name() == Some(OsStr::new("deps"))
        && let Some(profile_dir) = current_dir.parent()
    {
        let candidate = profile_dir.join(worker_binary_name());
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!(
        "could not locate whepprobe beside {} or one level above a deps directory; set {WORKER_ENV}",
        current_exe.display()
    )
}

/// Entry point for whepprobe's hidden worker mode. The caller configures logs
/// for stderr only: stdout is exclusively versioned binary framing.
pub async fn run_stdio() -> Result<()> {
    run_worker(tokio::io::stdin(), tokio::io::stdout()).await
}

async fn run_worker<R, W>(mut input: R, mut output: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let response = match decode_frames(&mut input).await {
        Ok(result) => WorkerResponse::Complete { result },
        Err(error) => WorkerResponse::Error {
            error: format!("{error:#}"),
        },
    };
    let payload = serde_json::to_vec(&response).context("failed to encode worker response")?;
    write_frame(&mut output, FrameKind::Response, &payload).await
}

async fn decode_frames<R>(input: &mut R) -> Result<DecodeSummary>
where
    R: AsyncRead + Unpin,
{
    let config = match read_frame(input).await? {
        Some(Frame {
            kind: FrameKind::Configure,
            payload,
        }) => serde_json::from_slice::<DecodeWorkerConfig>(&payload)
            .context("invalid decode-worker configuration")?,
        Some(frame) => bail!(
            "decode worker expected configuration, received {:?}",
            frame.kind
        ),
        None => bail!("decode worker received EOF before configuration"),
    };
    let is_audio = config.mime_type.to_ascii_lowercase().starts_with("audio/");
    let mut decoder = RtpFrameDecoder::new(config.mime_type, config.sprop_params.as_deref())?;

    loop {
        match read_frame(input).await? {
            Some(Frame {
                kind: FrameKind::Rtp,
                payload,
            }) => decoder.feed_rtp(&payload)?,
            Some(frame) => bail!(
                "decode worker expected RTP or EOF, received {:?}",
                frame.kind
            ),
            None => {
                return if is_audio {
                    decoder.finish_audio().map(DecodeSummary::Audio)
                } else {
                    let (width, height, frame_count) = decoder.finish()?;
                    Ok(DecodeSummary::Video(VideoDecodeResult {
                        width,
                        height,
                        frame_count,
                    }))
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: FrameKind, len: u32) -> Vec<u8> {
        let mut header = FRAME_MAGIC.to_vec();
        header.push(FRAME_VERSION);
        header.push(kind as u8);
        header.extend_from_slice(&len.to_be_bytes());
        header
    }

    #[tokio::test]
    async fn framing_rejects_truncated_header_and_payload() {
        let error = read_frame(&mut &FRAME_MAGIC[..]).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("truncated decode-worker frame header")
        );

        let mut input = header(FrameKind::Rtp, 4);
        input.extend_from_slice(&[1, 2]);
        let error = read_frame(&mut input.as_slice()).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("truncated decode-worker frame payload")
        );
    }

    #[tokio::test]
    async fn framing_rejects_oversize_before_allocating_or_reading_payload() {
        let input = header(FrameKind::Rtp, MAX_RTP_FRAME_SIZE + 1);
        let error = read_frame(&mut input.as_slice()).await.unwrap_err();
        assert!(error.to_string().contains("limit"));

        let error = encode_frame(
            FrameKind::Configure,
            &vec![0; MAX_CONTROL_FRAME_SIZE as usize + 1],
        )
        .unwrap_err();
        assert!(error.to_string().contains("limit"));
    }

    #[tokio::test]
    async fn framing_distinguishes_clean_eof_and_rejects_other_versions() {
        assert!(read_frame(&mut &[][..]).await.unwrap().is_none());

        let mut input = header(FrameKind::Rtp, 0);
        input[2] = FRAME_VERSION + 1;
        let error = read_frame(&mut input.as_slice()).await.unwrap_err();
        assert!(error.to_string().contains("protocol version"));
    }

    #[test]
    fn worker_path_prefers_override_then_sibling_then_deps_parent() {
        let temp = tempfile::tempdir().unwrap();
        let deps = temp.path().join("deps");
        std::fs::create_dir(&deps).unwrap();
        let current = deps.join(format!("livewrk{}", std::env::consts::EXE_SUFFIX));
        let override_path = temp.path().join("override");
        let sibling = deps.join(worker_binary_name());
        let parent = temp.path().join(worker_binary_name());
        std::fs::write(&sibling, b"").unwrap();
        std::fs::write(&parent, b"").unwrap();

        assert_eq!(
            resolve_worker_binary_from(Some(override_path.clone().into_os_string()), &current)
                .unwrap(),
            override_path
        );
        assert_eq!(resolve_worker_binary_from(None, &current).unwrap(), sibling);
        std::fs::remove_file(deps.join(worker_binary_name())).unwrap();
        assert_eq!(resolve_worker_binary_from(None, &current).unwrap(), parent);
    }

    #[cfg(unix)]
    mod process {
        use std::os::unix::fs::PermissionsExt;

        use super::*;

        struct FakeWorker {
            _temp: tempfile::TempDir,
            path: PathBuf,
            pid_path: PathBuf,
        }

        fn unresponsive_worker() -> FakeWorker {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("fake-whepprobe");
            let pid_path = temp.path().join("pid");
            let quote = |path: &Path| path.to_string_lossy().replace('\'', "'\\''");
            let script = format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec sleep 60\n",
                quote(&pid_path)
            );
            std::fs::write(&path, script).unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
            FakeWorker {
                _temp: temp,
                path,
                pid_path,
            }
        }

        async fn pid(fake: &FakeWorker) -> u32 {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(pid) = tokio::fs::read_to_string(&fake.pid_path).await
                        && let Ok(pid) = pid.parse()
                    {
                        return pid;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("fake worker did not write its pid")
        }

        fn config() -> DecodeWorkerConfig {
            DecodeWorkerConfig {
                mime_type: "audio/opus".to_string(),
                sprop_params: None,
            }
        }

        fn assert_killed_and_reaped(pid: u32) {
            // The reaper thread polls the killed child, so reaping is
            // asynchronous with respect to `Drop`; give it a bounded window.
            for _ in 0..200 {
                let status = std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stderr(Stdio::null())
                    .status()
                    .unwrap();
                if !status.success() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("worker {pid} is still alive or a zombie");
        }

        #[tokio::test]
        async fn unresponsive_worker_is_killed_and_reaped_on_deadline() {
            let fake = unresponsive_worker();
            let ct = CancellationToken::new();
            let worker = DecodeWorker::start_at(
                fake.path.clone(),
                config(),
                &ct,
                Instant::now() + Duration::from_secs(2),
            )
            .await
            .unwrap();
            let pid = pid(&fake).await;
            let error = worker
                .finish(&ct, Instant::now() + Duration::from_millis(50))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("timed out"), "{error:#}");
            assert_killed_and_reaped(pid);
        }

        #[tokio::test]
        async fn dropping_outer_finish_future_kills_and_reaps_worker() {
            let fake = unresponsive_worker();
            let ct = CancellationToken::new();
            let worker = DecodeWorker::start_at(
                fake.path.clone(),
                config(),
                &ct,
                Instant::now() + Duration::from_secs(2),
            )
            .await
            .unwrap();
            let pid = pid(&fake).await;
            {
                let finish = worker.finish(&ct, Instant::now() + Duration::from_secs(30));
                tokio::pin!(finish);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                    result = &mut finish => panic!("unresponsive worker returned early: {result:?}"),
                }
            }
            assert_killed_and_reaped(pid);
        }

        #[tokio::test]
        async fn blocked_send_observes_cancellation_and_reaps_worker() {
            let fake = unresponsive_worker();
            let ct = CancellationToken::new();
            let mut worker = DecodeWorker::start_at(
                fake.path.clone(),
                config(),
                &ct,
                Instant::now() + Duration::from_secs(2),
            )
            .await
            .unwrap();
            let pid = pid(&fake).await;
            let cancel_ct = ct.clone();
            let cancel = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                cancel_ct.cancel();
            });
            let packet = vec![0; MAX_RTP_FRAME_SIZE as usize];
            let result = worker
                .send_rtp(&packet, &ct, Instant::now() + Duration::from_secs(30))
                .await;
            cancel.await.unwrap();
            assert!(result.is_err());
            assert_killed_and_reaped(pid);
        }

        #[tokio::test]
        async fn oversized_send_is_rejected_and_reaps_worker() {
            let fake = unresponsive_worker();
            let ct = CancellationToken::new();
            let mut worker = DecodeWorker::start_at(
                fake.path.clone(),
                config(),
                &ct,
                Instant::now() + Duration::from_secs(2),
            )
            .await
            .unwrap();
            let pid = pid(&fake).await;
            let packet = vec![0; MAX_RTP_FRAME_SIZE as usize + 1];
            let error = worker
                .send_rtp(&packet, &ct, Instant::now() + Duration::from_secs(30))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("limit"));
            assert_killed_and_reaped(pid);
        }
    }
}
