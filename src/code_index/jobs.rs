//! GUI TaskExecutor 与 MCP 共用索引专用任务池，仓库任务独立排队和退避。

use rayon::{ThreadPool, ThreadPoolBuilder};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub fn index_task_pool() -> Arc<ThreadPool> {
    static POOL: OnceLock<Arc<ThreadPool>> = OnceLock::new();
    Arc::clone(POOL.get_or_init(|| {
        Arc::new(
            ThreadPoolBuilder::new()
                .num_threads(2)
                .thread_name(|index| format!("khaslana-index-{index}"))
                .build()
                .expect("无法创建索引任务池"),
        )
    }))
}

/// 多仓库共用计算池，避免每个仓库各自创建一组 CPU 工作线程。
pub(super) fn compute_pool() -> &'static ThreadPool {
    static POOL: OnceLock<ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| ThreadPoolBuilder::new()
        .num_threads(worker_count())
        .thread_name(|index| format!("khaslana-index-compute-{index}"))
        .build().expect("无法创建索引计算池"))
}

pub(super) fn worker_count() -> usize {
    // 每个解析器可能持有语法树和缓冲；按解析内存预算限制高核心机器的并发。
    std::thread::available_parallelism().map_or(2, |count| count.get())
        .saturating_sub(1).clamp(1, 32)
}

/// GUI、MCP 和提交快照按数据库身份互斥，不阻塞其他仓库。
pub(super) fn database_lock(path: &std::path::Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>> = OnceLock::new();
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| {
        let parent = path.parent().and_then(|parent| std::fs::canonicalize(parent).ok());
        parent.map_or_else(|| path.to_path_buf(), |parent| parent.join(path.file_name().unwrap_or_default()))
    });
    let key = absolute.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let key = key.to_lowercase();
    let mut locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|error| error.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) { return lock; }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

#[derive(Default)]
struct JobState {
    active: bool,
    running: bool,
    retry_at: Option<Instant>,
    failures: u32,
    last_error: Option<String>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub(super) struct IndexCoordinator {
    states: Arc<(Mutex<HashMap<String, JobState>>, Condvar)>,
    stopped: Arc<AtomicBool>,
}

impl IndexCoordinator {
    pub fn schedule<F>(&self, key: String, force: bool, task: F) -> bool
    where
        F: FnOnce(Arc<AtomicBool>) -> std::result::Result<(), String> + Send + 'static,
    {
        if self.stopped.load(Ordering::Relaxed) {
            return false;
        }
        let cancel = {
            let mut states = self.states.0.lock().unwrap_or_else(|e| e.into_inner());
            if self.stopped.load(Ordering::Relaxed) {
                return false;
            }
            let state = states.entry(key.clone()).or_default();
            if state.active || (!force && state.retry_at.is_some_and(|time| time > Instant::now()))
            {
                return false;
            }
            state.active = true;
            state.running = false;
            state.cancel = Arc::new(AtomicBool::new(false));
            Arc::clone(&state.cancel)
        };
        let coordinator = self.clone();
        index_task_pool().spawn(move || {
            if let Some(state) = coordinator
                .states
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(&key)
            {
                state.running = true;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if cancel.load(Ordering::Relaxed) || coordinator.stopped.load(Ordering::Relaxed) {
                    return Err("索引任务已取消".into());
                }
                task(cancel)
            }))
            .unwrap_or_else(|payload| {
                Err(payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| {
                        payload
                            .downcast_ref::<&str>()
                            .map(|message| message.to_string())
                    })
                    .unwrap_or_else(|| "索引任务异常退出".into()))
            });
            let mut states = coordinator
                .states
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(state) = states.get_mut(&key) {
                state.active = false;
                state.running = false;
                match result {
                    Ok(()) => {
                        state.failures = 0;
                        state.last_error = None;
                        state.retry_at = Some(Instant::now() + Duration::from_secs(2));
                    }
                    Err(error) => {
                        state.failures = state.failures.saturating_add(1);
                        state.last_error = Some(error);
                        state.retry_at =
                            Some(Instant::now() + Duration::from_secs(1 << state.failures.min(5)));
                    }
                }
            }
            coordinator.states.1.notify_all();
        });
        true
    }

    pub fn wait(&self, key: &str) {
        let states = self.states.0.lock().unwrap_or_else(|e| e.into_inner());
        drop(
            self.states
                .1
                .wait_while(states, |states| {
                    states.get(key).is_some_and(|state| state.active)
                })
                .unwrap_or_else(|e| e.into_inner()),
        );
    }

    pub fn status(&self, key: &str) -> Value {
        let states = self.states.0.lock().unwrap_or_else(|e| e.into_inner());
        match states.get(key) {
            Some(state) => {
                json!({ "active": state.active, "phase": if state.running { "running" } else if state.active { "queued" } else { "idle" },
                "failures": state.failures, "last_error": state.last_error,
                "retry_after_ms": state.retry_at.map(|time| time.saturating_duration_since(Instant::now()).as_millis() as u64).unwrap_or(0) })
            }
            None => json!({ "active": false, "phase": "idle", "failures": 0, "retry_after_ms": 0 }),
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        for state in self
            .states
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            state.cancel.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(super) fn expire_delay(&self, key: &str) {
        if let Some(state) = self
            .states
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(key)
        {
            state.retry_at = None;
        }
    }
}
