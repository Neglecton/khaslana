use std::sync::mpsc;

use super::*;

#[test]
fn task_executor_runs_short_and_long_tasks() {
    let executor = TaskExecutor::new();
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
