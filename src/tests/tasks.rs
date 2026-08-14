use std::sync::mpsc;

use super::*;

#[test]
fn task_executor_runs_short_and_long_tasks() {
    let (event_tx, _event_rx) = async_channel::unbounded();
    let executor = TaskExecutor::new(event_tx);
    let (tx, rx) = mpsc::channel();
    executor.spawn(TaskKind::Short, {
        let tx = tx.clone();
        move || tx.send("short").unwrap()
    });
    executor.spawn(TaskKind::Long, move || tx.send("long").unwrap());

    let mut values = vec![rx.recv().unwrap(), rx.recv().unwrap()];
    values.sort();
    assert_eq!(values, ["long", "short"]);
}

// panic 的任务被 catch_unwind 捕获并向 UI 通道发 BackgroundTaskPanicked，
// 而不是被 rayon 静默吞掉（那会让 busy/加载标志永久卡死）。
#[test]
fn task_executor_reports_panicked_task() {
    let (event_tx, event_rx) = async_channel::unbounded();
    let executor = TaskExecutor::new(event_tx);
    let (done_tx, done_rx) = mpsc::channel();
    executor.spawn(TaskKind::Short, move || {
        let _ = done_tx.send(());
        panic!("任务panic消息");
    });
    done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();

    // async-channel 的阻塞接收没有超时变体，轮询 try_recv 等待事件。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let event = loop {
        match event_rx.try_recv() {
            Ok(event) => break event,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => panic!("5 秒内未收到 BackgroundTaskPanicked 事件"),
        }
    };
    match event {
        UiEvent::BackgroundTaskPanicked { message } => {
            assert!(message.contains("任务panic消息"));
        }
        other => panic!("期待 BackgroundTaskPanicked，实际是 {other:?}"),
    }
}
