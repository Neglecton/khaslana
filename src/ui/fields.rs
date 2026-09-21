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
//! 迁移范围是 [`kit_field_migrated`]：静态字段（`DEDICATED_FIELDS`）里除冲突草稿
//! 外的全部。工作流动态字段与冲突草稿留在旧自绘路径，`input()` 对未建宿主的字段
//! 自动回退，所以这里可以按字段逐批放开。新增静态 `FieldId` 会自动进入迁移范围
//! （`ensure_kit_fields` 遍历的是注册表本身，不是手写清单）。
//!
//! 两处容易踩的地方：`focused_field` 必须先让位给 Kit 字段（`kit_field_focused`），
//! 否则同一个按键会被 Kit 与自绘 action 各写一遍；`Change` 订阅要调
//! `notify_text_field_changed`，与自绘 `text_*` handler 保持同一通知点。

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable, FocusHandle,
    SharedString, Subscription, Window, prelude::*, px,
};
use gpui_kit::component::input::{AnyInputState, Input, InputEvent, InputState, Textarea, TextareaState};

use crate::{FieldId, RepositoryView};

/// 单行输入框高度（与设置页其余控件同一档）。
const KIT_INPUT_HEIGHT: f32 = 34.0;
/// 紧凑档单行高度（列表标题行内的过滤框等）。
const KIT_INPUT_COMPACT_HEIGHT: f32 = 28.0;
/// 多行输入框的固定可视高度：5 行 × 18px 行高，与旧自绘框同尺寸，超出滚动。
const KIT_TEXTAREA_HEIGHT: f32 = 92.0;

/// 该字段是否在 Kit 输入迁移范围内。
///
/// 冲突草稿（`ConflictEditor`）是带按块接受、语法高亮与三栏联动的领域编辑器，
/// 按计划留到 M6 与冲突工作台一起处理；工作流动态字段随工作流页在 M6 迁移。
/// 其余静态字段一律迁到 Kit——新增 `FieldId` 不需要改这里。
pub(crate) fn kit_field_migrated(id: FieldId) -> bool {
    !matches!(
        id,
        FieldId::ConflictEditor | FieldId::WorkflowInput(_) | FieldId::WorkflowEditor(_)
    )
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
    /// 持有订阅本身：drop 即退订。字段与 `RepositoryView` 同生命周期。
    _subscriptions: Vec<Subscription>,
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
    }

    fn spawn_kit_field(&mut self, id: FieldId, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.field(id).placeholder.clone();
        let secret = self.field(id).secret;
        let initial = self.field(id).value.clone();
        let multiline = Self::is_multiline_field(id);

        let input = if multiline {
            KitFieldInput::Multi(
                cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder)),
            )
        } else {
            KitFieldInput::Single(cx.new(|cx| {
                let state = InputState::new(window, cx).placeholder(placeholder);
                if secret {
                    state.masked(true)
                } else {
                    state
                }
            }))
        };
        input.set_value(initial, window, cx);

        // 把表单真值的 focus handle 指向 Kit 输入：既有的
        // `window.focus(&self.field(id).focus, cx)` 调用点（弹窗打开自动聚焦、
        // 搜索入口等）因此零改动地落到 Kit 输入框上。
        self.field_mut(id).focus = input.focus_handle(cx);

        let subscriptions = match &input {
            KitFieldInput::Single(state) => {
                subscribe_kit_input(state, window, cx, id, multiline)
            }
            KitFieldInput::Multi(state) => subscribe_kit_input(state, window, cx, id, multiline),
        };

        self.kit_fields.push((
            id,
            KitField {
                input,
                _subscriptions: subscriptions,
            },
        ));
    }

    /// 程序回填 → Kit：只在两侧不一致时写入，值稳定后不再触碰（不会重置光标）。
    fn sync_kit_field(&mut self, id: FieldId, window: &mut Window, cx: &mut Context<Self>) {
        let truth = self.field(id).value.clone();
        let Some(field) = self.kit_field(id) else {
            return;
        };
        if field.value(cx).as_ref() == truth {
            return;
        }
        field.set_value(truth, window, cx);
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
            if this.field(id).value == value {
                return;
            }
            this.field_mut(id).set_value(value);
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
        move |this, _entity, event: &InputEvent, _window, cx| {
            let InputEvent::PressEnter { .. } = event else {
                return;
            };
            if !kit_press_enter_submits(multiline) {
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
