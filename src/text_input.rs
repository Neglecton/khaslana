//! 表单字段的业务真值容器（Kit 输入适配层的真值侧）。
//!
//! M7 起全部输入框的渲染、光标、选区、IME 与剪贴板都由 Kit 的
//! `InputState` / `TextareaState` 承担（见 `ui/fields.rs`）：自绘输入元素、
//! 自绘 `text_*` action 与 `EntityInputHandler` 通路已随迁移完成删除。
//! 这里只剩业务真值——`TextFieldState` 仍被全项目约两百处表单读写点
//! （`xxx_form_settings()`、`save_*_from_form()`、回填点）直接访问，
//! Kit 宿主每帧经 `ensure_kit_fields` 与它对齐。
//!
//! **不要**把真值与 Kit 的 `InputState` 合成一份：前者是业务数据，
//! 后者是渲染态，两者的双向同步规则写在 `ui/fields.rs` 模块文档。

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{Context, FocusHandle, SharedString};

use crate::RepositoryView;

/// 进程内自增的文本框身份。同一位置（`FieldId` 下标/槽位）的业务真值被
/// 整体更换后——工作流模板加载、编辑器重建、AI 回填、步骤交换——即使文本
/// 恰好相同，Kit 宿主也据此完整重绑，不把上一个对象的 placeholder、焦点
/// 指向与撤销历史串给新字段（审查 R5）。
static NEXT_TEXT_FIELD_UID: AtomicU64 = AtomicU64::new(1);

/// 编辑状态的真值部分（`TextFieldState` 经 `Deref` 暴露这些字段）。
#[derive(Clone, Debug)]
pub(crate) struct TextEditState {
    pub(crate) value: String,
    pub(crate) secret: bool,
    pub(crate) caret: usize,
    pub(crate) selection_anchor: Option<usize>,
}

impl TextEditState {
    pub(crate) fn new() -> Self {
        Self {
            value: String::new(),
            secret: false,
            caret: 0,
            selection_anchor: None,
        }
    }

    /// 程序回填：值整体替换，光标落到末尾（Kit 侧由 `sync_kit_field`
    /// 在两侧不一致时推送，不会触发 `Change` 回流）。
    pub(crate) fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.caret = self.value.len();
        self.selection_anchor = None;
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
        self.caret = 0;
        self.selection_anchor = None;
    }
}

/// 表单文本框的业务真值：焦点句柄 + 占位符 + 编辑状态。
///
/// `Deref`/`DerefMut` 到 `TextEditState` 让既有 `field.value` / `field.caret`
/// 等读写点零改动。
#[derive(Clone, Debug)]
pub(crate) struct TextFieldState {
    pub(crate) focus: FocusHandle,
    pub(crate) placeholder: SharedString,
    /// 构造时分配的身份：随对象移动（Vec 重排不改变它），业务对象被整体
    /// 更换时必然不同。Kit 宿主据此判断「同一个输入框」还是「同一个位置
    /// 换了内容」——后者必须完整重绑，不能以字符串相同跳过。
    pub(crate) uid: u64,
    edit: TextEditState,
}

impl TextFieldState {
    pub(crate) fn new(
        cx: &mut Context<RepositoryView>,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            placeholder: placeholder.into(),
            uid: NEXT_TEXT_FIELD_UID.fetch_add(1, Ordering::Relaxed),
            edit: TextEditState::new(),
        }
    }

    pub(crate) fn with_value(mut self, value: impl Into<String>) -> Self {
        self.edit.set_value(value);
        self
    }

    pub(crate) fn secret(mut self) -> Self {
        self.edit.secret = true;
        self
    }
}

impl Deref for TextFieldState {
    type Target = TextEditState;

    fn deref(&self) -> &Self::Target {
        &self.edit
    }
}

impl DerefMut for TextFieldState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.edit
    }
}
