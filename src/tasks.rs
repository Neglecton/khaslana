use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use rayon::{ThreadPool, ThreadPoolBuilder};

use crate::UiEvent;

#[derive(Clone)]
pub(crate) struct TaskExecutor {
    short_pool: Arc<ThreadPool>,
    long_pool: Arc<ThreadPool>,
    event_tx: async_channel::Sender<UiEvent>,
}

impl TaskExecutor {
    pub(crate) fn new(event_tx: async_channel::Sender<UiEvent>) -> Self {
        let short_threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(4)
            .clamp(2, 4);
        let long_threads = 2;
        Self {
            short_pool: Arc::new(build_pool("khaslana-short", short_threads)),
            long_pool: Arc::new(build_pool("khaslana-long", long_threads)),
            event_tx,
        }
    }

    pub(crate) fn spawn<F>(&self, kind: TaskKind, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        // rayon 会捕获并吞掉任务 panic：既不回发任何 UiEvent，也不复位 busy/
        // 加载标志，对应 tab 的状态机会永久卡死。这里统一包一层 catch_unwind，
        // panic 时向 UI 发事件兜底复位并提示。
        let tx = self.event_tx.clone();
        let wrapped = move || {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(task)) {
                let message = panic_message(payload);
                tracing::error!(target: "khaslana::tasks", "后台任务 panic：{message}");
                // try_send：rayon 池线程不能阻塞等待异步 send。
                let _ = tx.try_send(UiEvent::BackgroundTaskPanicked { message });
            }
        };
        match kind {
            TaskKind::Short => self.short_pool.spawn(wrapped),
            TaskKind::Long => self.long_pool.spawn(wrapped),
        }
    }
}

/// 从 catch_unwind 的 payload 提取可读的 panic 信息。
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "未知原因".to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskKind {
    Short,
    Long,
}

fn build_pool(name: &'static str, threads: usize) -> ThreadPool {
    ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |index| format!("{name}-{index}"))
        .build()
        .expect("failed to build Khaslana task pool")
}

#[cfg(test)]
#[path = "tests/tasks.rs"]
mod tests;
