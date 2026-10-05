use khaslana::AiProviderSettings;

#[derive(Default)]
pub(crate) enum ConnectionTestOutcome {
    #[default]
    Idle,
    Running,
    Passed(String),
    Failed(String),
}

/// 连接测试只展示同一份参数的结果，不能借用全局 Git 操作的状态。
#[derive(Default)]
pub(crate) struct AiConnectionTestState {
    request_id: u64,
    settings: Option<AiProviderSettings>,
    outcome: ConnectionTestOutcome,
}

impl AiConnectionTestState {
    pub(crate) fn begin(&mut self, settings: AiProviderSettings) -> u64 {
        self.request_id = self.request_id.wrapping_add(1);
        self.settings = Some(settings);
        self.outcome = ConnectionTestOutcome::Running;
        self.request_id
    }

    pub(crate) fn finish(&mut self, request_id: u64, result: Result<String, String>) -> bool {
        if request_id != self.request_id || !self.is_running() {
            return false;
        }
        self.outcome = match result {
            Ok(message) => ConnectionTestOutcome::Passed(message),
            Err(error) => ConnectionTestOutcome::Failed(error),
        };
        true
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self.outcome, ConnectionTestOutcome::Running)
    }

    /// 调度器全局复位后，旧连接事件不能再解锁后来开始的操作。
    pub(crate) fn interrupt(&mut self, error: String) {
        if self.is_running() {
            self.request_id = self.request_id.wrapping_add(1);
            self.outcome = ConnectionTestOutcome::Failed(error);
        }
    }

    pub(crate) fn outcome_for(&self, settings: &AiProviderSettings) -> &ConnectionTestOutcome {
        if self.settings.as_ref() == Some(settings) {
            &self.outcome
        } else {
            &ConnectionTestOutcome::Idle
        }
    }
}
