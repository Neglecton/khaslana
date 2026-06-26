use std::sync::Arc;

use rayon::{ThreadPool, ThreadPoolBuilder};

#[derive(Clone)]
pub(crate) struct TaskExecutor {
    short_pool: Arc<ThreadPool>,
    long_pool: Arc<ThreadPool>,
}

impl TaskExecutor {
    pub(crate) fn new() -> Self {
        let short_threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(4)
            .clamp(2, 4);
        let long_threads = 2;
        Self {
            short_pool: Arc::new(build_pool("khaslana-short", short_threads)),
            long_pool: Arc::new(build_pool("khaslana-long", long_threads)),
        }
    }

    pub(crate) fn spawn<F>(&self, kind: TaskKind, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        match kind {
            TaskKind::Short => self.short_pool.spawn(task),
            TaskKind::Long => self.long_pool.spawn(task),
        }
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
