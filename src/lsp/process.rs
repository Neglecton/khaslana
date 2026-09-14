use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{LspClient, ServerRequestPolicy};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub env_remove: Vec<String>,
    pub current_dir: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("启动 LSP 进程失败：{0}")]
    Spawn(#[from] std::io::Error),
    #[error("LSP 子进程缺少 {0} 管道")]
    MissingPipe(&'static str),
}

pub struct OwnedLspProcess {
    child: Child,
    client: LspClient,
    stderr_tail: Arc<Mutex<Vec<String>>>,
    #[cfg(windows)]
    _job: OwnedJob,
}

impl OwnedLspProcess {
    pub fn spawn(spec: &LaunchSpec, policy: ServerRequestPolicy) -> Result<Self, ProcessError> {
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(current_dir) = &spec.current_dir {
            command.current_dir(current_dir);
        }
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        for name in &spec.env_remove {
            command.env_remove(name);
        }
        // JDT 的协议走管道，Windows 不应弹出控制台窗口。
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = match OwnedJob::assign(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProcessError::Spawn(error));
            }
        };
        let stdin = child
            .stdin
            .take()
            .ok_or(ProcessError::MissingPipe("stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(ProcessError::MissingPipe("stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or(ProcessError::MissingPipe("stderr"))?;
        let client = LspClient::start(stdout, stdin, policy);
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_sink = Arc::clone(&stderr_tail);
        thread::Builder::new()
            .name("khaslana-lsp-stderr".to_string())
            .spawn(move || {
                use std::io::{BufRead, BufReader};
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    tracing::debug!(target: "lsp_stderr", "{line}");
                    let mut tail = stderr_sink.lock().expect("LSP stderr 锁被污染");
                    if tail.len() == 100 {
                        tail.remove(0);
                    }
                    tail.push(line);
                }
            })
            .expect("无法启动 LSP stderr 线程");
        Ok(Self {
            child,
            client,
            stderr_tail,
            #[cfg(windows)]
            _job: job,
        })
    }

    pub fn client(&self) -> &LspClient {
        &self.client
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail
            .lock()
            .expect("LSP stderr 锁被污染")
            .clone()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub fn shutdown(&mut self, timeout: Duration) {
        let rpc_timeout = timeout.min(Duration::from_secs(2));
        let _ = self.client.shutdown(rpc_timeout);
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(windows)]
struct OwnedJob(windows_sys::Win32::Foundation::HANDLE);

// Job HANDLE 是由本结构独占的内核句柄，跨线程移动所有权不会改变其有效性；
// Drop 仍只会关闭一次。这样语义服务可以安全放进后台 AI 任务持有的 Arc。
#[cfg(windows)]
unsafe impl Send for OwnedJob {}

#[cfg(windows)]
impl OwnedJob {
    fn assign(child: &Child) -> std::io::Result<Self> {
        use std::mem::{size_of, zeroed};
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // SAFETY: 所有结构先清零再填写文档要求的 limit flag；句柄有效性逐步检查，
        // 失败路径立即 CloseHandle。Child 的进程句柄在 assign 调用期间有效。
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() || job == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&information as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Self(job))
        }
    }
}

#[cfg(windows)]
impl Drop for OwnedJob {
    fn drop(&mut self) {
        // SAFETY: 句柄由 CreateJobObjectW 创建且仅由本对象关闭一次。
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

impl Drop for OwnedLspProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
