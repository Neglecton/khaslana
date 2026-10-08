//! V2 动作参数适配。已识别字段进入表单，其余字段保留在高级参数中，避免重存丢失守卫。
use super::*;
use std::collections::HashMap;
use serde_json::{Map, Value};

pub(super) const INVOKE_SLOTS: &[WorkflowStepSlot] = &[
    WorkflowStepSlot::StepId, WorkflowStepSlot::Uses, WorkflowStepSlot::Arguments,
    WorkflowStepSlot::SaveAs, WorkflowStepSlot::Server, WorkflowStepSlot::Tool,
    WorkflowStepSlot::ToolArguments, WorkflowStepSlot::Script, WorkflowStepSlot::JsInput,
    WorkflowStepSlot::Skill, WorkflowStepSlot::Task, WorkflowStepSlot::Tools,
];

pub(super) fn configured_server_tools(server: &khaslana::workflow::extensions::WorkflowMcpServer) -> Vec<String> {
    if !server.enabled { return Vec::new(); }
    // 缓存只提供选择名称；白名单模式不能因缓存扩大可选范围，执行时仍由宿主校验。
    server.tools.keys().cloned()
        .chain(server.cached_tools.iter().filter(|_| server.auto_discover).cloned())
        .collect::<std::collections::BTreeSet<_>>().into_iter().collect()
}

#[derive(Clone, Debug)]
pub(crate) struct InvokeEditorData {
    fields: HashMap<WorkflowStepSlot, String>,
}

impl Default for InvokeEditorData {
    fn default() -> Self {
        let mut fields = HashMap::new();
        fields.insert(WorkflowStepSlot::Arguments, "{}".into());
        Self { fields }
    }
}

impl InvokeEditorData {
    pub(super) fn value(&self, slot: WorkflowStepSlot) -> &str {
        self.fields.get(&slot).map(String::as_str).unwrap_or("")
    }

    pub(super) fn set(&mut self, slot: WorkflowStepSlot, value: String) {
        self.fields.insert(slot, value);
    }

    pub(super) fn parameters(&self) -> Vec<(WorkflowStepSlot, &'static str, bool)> {
        use WorkflowStepSlot::*;
        match self.value(Uses) {
            "mcp.call" => vec![(Server, "server", false), (Tool, "tool", false), (ToolArguments, "arguments", true)],
            "js.run" => vec![(Script, "script", false), (JsInput, "input", true), (Tools, "tools", true)],
            "skill.run" => vec![(Skill, "skill", false), (Task, "task", false), (Tools, "tools", true)],
            _ => vec![],
        }
    }

    pub(super) fn from_step(id: &str, uses: &str, arguments: &Value, save_as: &Option<String>) -> Self {
        use WorkflowStepSlot::*;
        let mut data = Self::default();
        data.set(StepId, id.into());
        data.set(Uses, uses.into());
        data.set(SaveAs, save_as.clone().unwrap_or_default());
        let mut extra = arguments.as_object().cloned().unwrap_or_default();
        for (slot, key, json) in data.parameters() {
            if let Some(value) = extra.get(key) {
                if json {
                    data.set(slot, serde_json::to_string_pretty(value).expect("JSON 值可序列化"));
                    extra.remove(key);
                } else if let Some(text) = value.as_str() {
                    data.set(slot, text.into());
                    extra.remove(key);
                }
            }
        }
        data.set(Arguments, serde_json::to_string_pretty(&extra).expect("JSON 对象可序列化"));
        data
    }

    pub(super) fn new_action(uses: &str, id: &str) -> Self {
        Self::from_step(id, uses, &Value::Object(Map::new()), &None)
    }

    pub(super) fn build(&self) -> Result<WorkflowStep, String> {
        use WorkflowStepSlot::*;
        let mut arguments: Value = json5::from_str(self.value(Arguments))
            .map_err(|error| format!("高级参数不是有效的 JSON5：{error}"))?;
        let object = arguments.as_object_mut().ok_or("高级参数必须是对象")?;
        for (slot, key, json) in self.parameters() {
            let text = self.value(slot);
            if !text.is_empty() {
                let value = if json {
                    json5::from_str(text).map_err(|error| format!("{}格式错误：{error}", slot.label()))?
                } else {
                    Value::String(text.into())
                };
                object.insert(key.into(), value);
            }
        }
        let required = match self.value(Uses) {
            "mcp.call" => &[Server, Tool][..],
            "js.run" => &[Script][..],
            "skill.run" => &[Skill, Task][..],
            _ => &[],
        };
        for slot in required {
            if self.value(*slot).trim().is_empty() { return Err(format!("请填写{}", slot.label())); }
        }
        Ok(WorkflowStep::Invoke {
            id: self.value(StepId).trim().into(),
            uses: self.value(Uses).trim().into(),
            arguments,
            save_as: (!self.value(SaveAs).trim().is_empty()).then(|| self.value(SaveAs).trim().into()),
        })
    }

    pub(super) fn title(&self) -> &str {
        match self.value(WorkflowStepSlot::Uses) {
            "mcp.call" => "MCP 工具",
            "js.run" => "JavaScript 脚本",
            "skill.run" => "AI Skill",
            other => other,
        }
    }
}

#[cfg(test)]
#[path = "../tests/workflow_editor_v2.rs"]
mod tests;
