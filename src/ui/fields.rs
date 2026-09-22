//! Kit 输入字段适配层（M4）。
//!
//! 迁移期间同一个字段有两套状态，分工写在下面，**不要**把它们合成一份：
//!
//! - `TextFieldState`（`RepositoryView.field()` / `field_mut()`）仍是业务真值：
//!   全项目约两百处读取点（`xxx_form_settings()`、`save_*_from_form()`）与回填点
//!   都直接读写它，迁渲染层时不动这些调用点。
//! - Kit 的 `Entity<InputState>` / `Entity<TextareaState>` 承担渲染、光标、选区、
//!   IME 与剪贴板；它由 [`RepositoryView::ensure_kit_fields`] 在每帧渲染前对齐。
//!
//! 两个方向都收敛在 `ensure_kit_fields`：用户编辑经 `InputEvent::Change` 写回
//! 表单真值；程序回填仍走原来的 `field_mut(id).set_value(..)`，下一帧发现两侧
//! 不一致时推给 Kit。之所以能这样双写而不打架，是因为 Kit 的 `set_value` 内部
//! 把 `emit_events` 置为 false（`gpui-base/src/input/base/state.rs`），程序回填
//! 不会再发 `Change`；而用户编辑写回的值在下一帧两侧已经相等，不会再触发
//! `set_value`（否则每次都把光标重置到末尾）。
//!
//! 迁移范围是 [`kit_field_migrated`]：**全部字段**。静态字段
//! （`DEDICATED_FIELDS`）里除冲突草稿焦点锚点外的全部自 M4 起迁入 Kit；
//! 工作流动态字段（`WorkflowInput` / `WorkflowEditor`）自 M6 起也迁入
//! Kit——它们不进静态注册表，由 [`RepositoryView::ensure_kit_fields`]
//! 末段的「动态字段多退少补」单独维护宿主生命周期（见
//! [`RepositoryView::reconcile_dynamic_kit_fields`]）。冲突草稿自 M6 起
//! 固定为只读文档视图，`ConflictEditor` 只剩 focus 锚点职责，不建任何
//! 输入宿主。新增静态 `FieldId` 会自动进入迁移范围（`ensure_kit_fields`
//! 遍历的是注册表本身，不是手写清单）。
//!
//! 业务对象被整体更换（模板加载 / 编辑器重建 / 步骤交换）时，宿主按
//! `TextFieldState` 的 uid 身份完整重绑（`rebind_kit_field`）：占位符、
//! 焦点指向与撤销历史都绑定在具体那份业务真值上，位置相同、文本也
//! 相同的不一定是同一个输入框（审查 R5）。
//!
//! 两处容易踩的地方：`focused_field` 必须先让位给 Kit 字段（`kit_field_focused`），
//! 否则同一个按键会被 Kit 与自绘 action 各写一遍；`Change` 订阅要调
//! `notify_text_field_changed`，与自绘 `text_*` handler 保持同一通知点。

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    SharedString, Subscription, Window, prelude::*, px,
};
use gpui_kit::component::input::{
    AnyInputState, Input, InputEvent, InputState, Textarea, TextareaState,
};

use crate::{FieldId, RepositoryView, workflow_editor::WorkflowEditorFieldId};

/// 单行输入框高度（与设置页其余控件同一档）。
const KIT_INPUT_HEIGHT: f32 = 34.0;
/// 紧凑档单行高度（列表标题行内的过滤框等）。
const KIT_INPUT_COMPACT_HEIGHT: f32 = 28.0;
/// 多行输入框的固定可视高度：5 行 × 18px 行高，与旧自绘框同尺寸，超出滚动。
const KIT_TEXTAREA_HEIGHT: f32 = 92.0;

/// 该字段是否在 Kit 输入迁移范围内。
///
/// 工作流动态字段（`WorkflowInput` / `WorkflowEditor`）按模板与编辑器状态动态增删，
/// 不在 `DEDICATED_FIELDS` 静态注册表里，由 [`RepositoryView::ensure_kit_fields`]
/// 末段的「动态字段多退少补」单独维护（见 [`RepositoryView::active_dynamic_field_ids`]）。
/// 冲突结果区自 M6 起固定为只读文档视图（`conflict_result_pane_uses_editor()` 恒
/// `false`），`ConflictEditor` 的自绘编辑器路径已删除，字段只剩 focus 锚点职责，
/// 不需要任何输入宿主。其余静态字段一律迁到 Kit——新增 `FieldId` 不需要改这里。
pub(crate) fn kit_field_migrated(id: FieldId) -> bool {
    !matches!(id, FieldId::ConflictEditor)
}

/// 字段的 Kit 输入宿主：单行 `Input` 或多行 `Textarea`。
pub(crate) enum KitFieldInput {
    Single(Entity<InputState>),
    Multi(Entity<TextareaState>),
}

impl KitFieldInput {
    fn value(&self, cx: &App) -> SharedString {
        match self {
            Self::Single(state) => state.read(cx).value(),
            Self::Multi(state) => state.read(cx).value(),
        }
    }

    fn set_value(&self, value: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        let value = value.into();
        match self {
            Self::Single(state) => state.update(cx, |state, cx| state.set_value(value, window, cx)),
            Self::Multi(state) => state.update(cx, |state, cx| state.set_value(value, window, cx)),
        }
    }

    fn set_placeholder(
        &self,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let placeholder = placeholder.into();
        match self {
            Self::Single(state) => state.update(cx, |state, cx| {
                state.set_placeholder(placeholder, window, cx)
            }),
            Self::Multi(state) => state.update(cx, |state, cx| {
                state.set_placeholder(placeholder, window, cx)
            }),
        }
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Single(state) => Focusable::focus_handle(state.read(cx), cx),
            Self::Multi(state) => Focusable::focus_handle(state.read(cx), cx),
        }
    }

    fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle(cx).is_focused(window)
    }
}

/// 一个字段的 Kit 输入宿主。
pub(crate) struct KitField {
    input: KitFieldInput,
    /// 该宿主当前服务的那份业务真值的身份。业务对象被整体更换（模板
    /// 加载 / 编辑器重建 / 步骤交换）后，即使 `FieldId` 位置与文本都
    /// 相同，也要完整重绑——否则上一个对象的 placeholder、焦点指向与
    /// 撤销历史会串到新字段上（审查 R5）。
    identity: KitFieldIdentity,
    /// 持有订阅本身：drop 即退订。字段与 `RepositoryView` 同生命周期。
    _subscriptions: Vec<Subscription>,
}

/// Kit 宿主的业务身份：真值侧 `TextFieldState` 的 uid + 占位符。
/// 两者任一变化都说明「同一个位置换了内容」，必须重绑。
#[derive(Clone, Debug, PartialEq, Eq)]
struct KitFieldIdentity {
    uid: u64,
    placeholder: SharedString,
}

/// 是否需要完整重绑：业务身份变化（对象整体更换 / 占位符变化）时为真。
///
/// 纯函数，供 [`RepositoryView::sync_kit_field`] 与单测共用。关键是 uid：
/// 两份业务字段文本恰好相同时，只有 uid 能区分「同一个输入框」与
/// 「同一个位置换了内容」——后者必须重绑（清撤销历史、重绑 placeholder
/// 与焦点），审查 R5 的验收场景正是「不同 label、相同初始文本」。
fn kit_field_needs_rebind(
    host: &KitFieldIdentity,
    truth_uid: u64,
    truth_placeholder: &SharedString,
) -> bool {
    host.uid != truth_uid || host.placeholder != *truth_placeholder
}

impl KitField {
    fn value(&self, cx: &App) -> SharedString {
        self.input.value(cx)
    }

    /// 程序回填。Kit 侧不会因此发 `Change`，不会回流到表单真值。
    fn set_value(&self, value: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        self.input.set_value(value, window, cx);
    }

    fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.input.is_focused(window, cx)
    }

    /// 渲染成 Kit 输入元素。`blocked` 来自操作遮罩（高风险操作期间禁止输入）。
    pub(crate) fn render(&self, compact: bool, blocked: bool) -> AnyElement {
        match &self.input {
            KitFieldInput::Single(state) => Input::new(state)
                .h(px(if compact {
                    KIT_INPUT_COMPACT_HEIGHT
                } else {
                    KIT_INPUT_HEIGHT
                }))
                .disabled(blocked)
                .into_any_element(),
            KitFieldInput::Multi(state) => Textarea::new(state)
                .h(px(KIT_TEXTAREA_HEIGHT))
                .disabled(blocked)
                .into_any_element(),
        }
    }
}

impl RepositoryView {
    /// 取得字段的 Kit 宿主；未迁移或尚未创建时为 `None`。
    pub(crate) fn kit_field(&self, id: FieldId) -> Option<&KitField> {
        self.kit_fields
            .iter()
            .find_map(|(field_id, field)| (*field_id == id).then_some(field))
    }

    /// 取得字段 Kit 宿主的可变引用（重绑时更新身份用）。
    fn kit_field_mut(&mut self, id: FieldId) -> Option<&mut KitField> {
        self.kit_fields
            .iter_mut()
            .find_map(|(field_id, field)| (*field_id == id).then_some(field))
    }

    /// 渲染前对齐 Kit 字段：缺的建好，被程序回填过的推给 Kit。
    ///
    /// 必须在 `RepositoryView::render` 顶部、任何渲染调用之前执行——渲染期只有
    /// `&self`，拿不到 `Context` 去建实体。
    pub(crate) fn ensure_kit_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (id, _) in crate::DEDICATED_FIELDS {
            let id = *id;
            if !kit_field_migrated(id) {
                continue;
            }
            if self.kit_field(id).is_none() {
                self.spawn_kit_field(id, window, cx);
            }
            self.sync_kit_field(id, window, cx);
        }
        self.reconcile_dynamic_kit_fields(window, cx);
    }

    /// 动态字段宿主的多退少补：宿主集合精确等于当前活跃的动态字段集合。
    ///
    /// 工作流动态字段的 `TextFieldState` 随模板加载/编辑器弹窗动态增删；宿主若
    /// 不跟着退场，孤儿宿主的 `Change` 事件会在字段消失后继续触发——严格寻址
    /// （`try_field_mut`）会让这些事件静默丢弃，但宿主列表本身仍会无限增长，
    /// 所以每帧先按 `active_dynamic_field_ids` 对齐集合，再逐个同步值。
    fn reconcile_dynamic_kit_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active: Vec<FieldId> = self.active_dynamic_field_ids();
        // 退场：宿主列表里不再活跃的字段整条移除（drop 即退订）。
        self.kit_fields.retain(|(id, _)| {
            !matches!(id, FieldId::WorkflowInput(_) | FieldId::WorkflowEditor(_))
                || active.contains(id)
        });
        // 进场：新出现的动态字段建宿主。静态注册表字段已在前一循环建过。
        for id in active {
            if self.kit_field(id).is_none() {
                self.spawn_kit_field(id, window, cx);
            }
            self.sync_kit_field(id, window, cx);
        }
    }

    /// 当前应存活的动态字段集合（工作流运行配置 + 编辑器弹窗字段）。
    ///
    /// 编辑器字段只枚举「已创建文本框」的那些——`ensure_workflow_editor_fields_inited`
    /// 在本函数之前运行，渲染需要的槽位都已就位；未初始化的槽位不会有宿主，
    /// `Change` 回调也因此寻址不到它们。
    fn active_dynamic_field_ids(&self) -> Vec<FieldId> {
        let mut ids: Vec<FieldId> = self
            .workflow_state
            .inputs
            .iter()
            .enumerate()
            .map(|(index, _)| FieldId::WorkflowInput(index))
            .collect();
        if self.workflow_editor.is_some() {
            ids.push(FieldId::WorkflowEditor(WorkflowEditorFieldId::Name));
            ids.push(FieldId::WorkflowEditor(WorkflowEditorFieldId::FileName));
            ids.push(FieldId::WorkflowEditor(
                WorkflowEditorFieldId::AiDescription,
            ));
            ids.push(FieldId::WorkflowEditor(WorkflowEditorFieldId::PickerSearch));
            // 步骤槽与变量行沿编辑器自身的寻址函数枚举已存在的文本框。
            if let Some(fields) = self.workflow_editor_live_fields() {
                ids.extend(fields);
            }
        }
        ids
    }

    /// 枚举编辑器当前已创建文本框的动态字段（步骤槽 / 输入变量行 / 自定义变量行）。
    fn workflow_editor_live_fields(&self) -> Option<Vec<FieldId>> {
        let state = self.workflow_editor.as_ref()?;
        let mut ids = Vec::new();
        for (step, step_state) in state.step_fields.iter().enumerate() {
            for slot in step_state.fields.keys() {
                ids.push(FieldId::WorkflowEditor(WorkflowEditorFieldId::StepParam {
                    step,
                    slot: *slot,
                }));
            }
        }
        for (index, row_state) in state.input_fields.iter().enumerate() {
            for (part, field) in [
                (
                    WorkflowEditorFieldId::InputPart {
                        index,
                        part: crate::workflow_editor::WorkflowInputPart::Key,
                    },
                    &row_state.key_field,
                ),
                (
                    WorkflowEditorFieldId::InputPart {
                        index,
                        part: crate::workflow_editor::WorkflowInputPart::Label,
                    },
                    &row_state.label_field,
                ),
                (
                    WorkflowEditorFieldId::InputPart {
                        index,
                        part: crate::workflow_editor::WorkflowInputPart::Description,
                    },
                    &row_state.description_field,
                ),
                (
                    WorkflowEditorFieldId::InputPart {
                        index,
                        part: crate::workflow_editor::WorkflowInputPart::Default,
                    },
                    &row_state.default_field,
                ),
            ] {
                if field.is_some() {
                    ids.push(FieldId::WorkflowEditor(part));
                }
            }
        }
        for (index, row_state) in state.var_fields.iter().enumerate() {
            if row_state.key_field.is_some() {
                ids.push(FieldId::WorkflowEditor(WorkflowEditorFieldId::VarPart {
                    index,
                    key: true,
                }));
            }
            if row_state.value_field.is_some() {
                ids.push(FieldId::WorkflowEditor(WorkflowEditorFieldId::VarPart {
                    index,
                    key: false,
                }));
            }
        }
        Some(ids)
    }

    fn spawn_kit_field(&mut self, id: FieldId, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.field(id).placeholder.clone();
        let secret = self.field(id).secret;
        let initial = self.field(id).value.clone();
        let uid = self.field(id).uid;
        let multiline = Self::is_multiline_field(id);
        let input = if multiline {
            KitFieldInput::Multi(
                cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder.clone())),
            )
        } else {
            KitFieldInput::Single(cx.new(|cx| {
                let state = InputState::new(window, cx).placeholder(placeholder.clone());
                if secret { state.masked(true) } else { state }
            }))
        };
        input.set_value(initial, window, cx);

        // 把表单真值的 focus handle 指向 Kit 输入：既有的
        // `window.focus(&self.field(id).focus, cx)` 调用点（弹窗打开自动聚焦、
        // 搜索入口等）因此零改动地落到 Kit 输入框上。
        self.field_mut(id).focus = input.focus_handle(cx);

        let subscriptions = match &input {
            KitFieldInput::Single(state) => subscribe_kit_input(state, window, cx, id, multiline),
            KitFieldInput::Multi(state) => subscribe_kit_input(state, window, cx, id, multiline),
        };

        self.kit_fields.push((
            id,
            KitField {
                input,
                identity: KitFieldIdentity { uid, placeholder },
                _subscriptions: subscriptions,
            },
        ));
    }

    /// 程序回填 → Kit：只在两侧不一致时写入，值稳定后不再触碰（不会重置光标）。
    ///
    /// 身份变化（业务对象被整体更换）走完整重绑而非普通回填：位置相同、
    /// 文本也相同的不一定是同一个输入框，placeholder、焦点指向与撤销历史
    /// 都绑定在具体那份业务真值上。
    fn sync_kit_field(&mut self, id: FieldId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(truth) = self.try_field(id) else {
            return;
        };
        let value = truth.value.clone();
        let identity = KitFieldIdentity {
            uid: truth.uid,
            placeholder: truth.placeholder.clone(),
        };
        let needs_rebind = match self.kit_field(id) {
            Some(field) => kit_field_needs_rebind(&field.identity, truth.uid, &truth.placeholder),
            None => return,
        };
        if needs_rebind {
            self.rebind_kit_field(id, value, identity, window, cx);
            return;
        }
        let Some(field) = self.kit_field(id) else {
            return;
        };
        if field.value(cx).as_ref() == value {
            return;
        }
        field.set_value(value, window, cx);
    }

    /// 业务对象更换后的完整重绑：占位符、焦点指向与值全部按新真值写入。
    ///
    /// 值也强制 `set_value`（即使文本相同）——它会清掉上一个业务对象留下
    /// 的撤销历史与选区，正是「Ctrl+Z 不回到前一模板」的关键。焦点重新
    /// 指向宿主：业务对象更换后 `field.focus` 是新建的句柄，不重指的话
    /// `window.focus(&field.focus)` 会落到一个没有元素跟踪的死句柄上。
    fn rebind_kit_field(
        &mut self,
        id: FieldId,
        value: String,
        identity: KitFieldIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.kit_field(id) else {
            return;
        };
        host.input
            .set_placeholder(identity.placeholder.clone(), window, cx);
        host.input.set_value(value, window, cx);
        let handle = host.input.focus_handle(cx);
        if let Some(field) = self.try_field_mut(id) {
            field.focus = handle;
        }
        if let Some(host) = self.kit_field_mut(id) {
            host.identity = identity;
        }
    }

    /// 当前聚焦的字段是否是已迁移到 Kit 的字段（键盘由 Kit 独占）。
    pub(crate) fn kit_field_focused(&self, window: &Window, cx: &App) -> bool {
        self.focused_kit_field(window, cx).is_some()
    }

    /// 返回当前聚焦的 Kit 字段，供跨字段的提交动作复用业务分发。
    pub(crate) fn focused_kit_field(&self, window: &Window, cx: &App) -> Option<FieldId> {
        self.kit_fields
            .iter()
            .find_map(|(id, field)| field.is_focused(window, cx).then_some(*id))
    }
}

/// 挂上 Kit 输入的 Change / PressEnter 订阅。
///
/// 泛型是为了让单行与多行共用一份逻辑：两者是不同的实体类型，但都发
/// [`InputEvent`]，且都能经 [`AnyInputState`] 读到值。
fn subscribe_kit_input<E>(
    entity: &Entity<E>,
    window: &mut Window,
    cx: &mut Context<RepositoryView>,
    id: FieldId,
    multiline: bool,
) -> Vec<Subscription>
where
    E: EventEmitter<InputEvent> + 'static,
    AnyInputState: From<Entity<E>>,
{
    let reader: AnyInputState = entity.clone().into();
    // 用户编辑 → 写回表单真值，业务读取点零改动。
    let change = cx.subscribe_in(
        entity,
        window,
        move |this, _entity, event: &InputEvent, _window, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let value = reader.value(cx).to_string();
            // 严格寻址：动态字段被重建/删除后，残留事件直接丢弃，
            // 不允许经越界兜底写进无关的静态字段（如 branch_name）。
            let Some(field) = this.try_field_mut(id) else {
                return;
            };
            if field.value == value {
                return;
            }
            field.set_value(value);
            // 与自绘路径（`text_*` handler）保持同一通知点：输入即查、
            // 工作流字段同步都挂在这里，漏掉会让这些字段迁到 Kit 后失灵。
            this.notify_text_field_changed(id);
            cx.notify();
        },
    );
    // 提交：单行框的 Enter 就是提交。多行框的普通 Enter 只负责换行；
    // Ctrl/Cmd+Enter 由 Input 上更高优先级的 TextSubmit 绑定在写入前截获，
    // 不能在 PressEnter 到达后再提交，否则 Kit 已经先插入了换行。
    let enter = cx.subscribe_in(
        entity,
        window,
        move |this, _entity, event: &InputEvent, window, cx| {
            let InputEvent::PressEnter { .. } = event else {
                return;
            };
            if !kit_press_enter_submits(multiline) {
                return;
            }
            // 严格寻址与 Change 同理：孤儿宿主的 Enter 不再触发任何提交。
            if this.try_field(id).is_none() {
                return;
            }
            // 单行 Enter 经 Kit 事件订阅到达，不经过根层 TextSubmit，
            // 必须同样阻止被新浮层盖住的输入框提交。
            if this.top_modal_focus_handle()
                .is_some_and(|handle| !handle.contains_focused(window, cx))
            {
                return;
            }
            this.submit_focused_field(id);
            cx.notify();
        },
    );
    vec![change, enter]
}

const fn kit_press_enter_submits(multiline: bool) -> bool {
    !multiline
}

#[cfg(test)]
#[path = "../tests/ui/fields.rs"]
mod tests;
