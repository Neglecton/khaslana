//! Focus Workbench 的应用壳层。
//!
//! 该模块只组织 titlebar 与 Context Navigator（模式按钮 + 仓库引用分组），
//! 不触碰各主页面内部布局。每次渲染都从 GPUI `Window::viewport_size()` 读取当前视口
//! 宽度，再交给纯函数决定壳层的信息密度；窗口创建时另有 `WindowOptions::window_min_size`
//! 保护原生控制区。

use gpui::{
    Context, CursorStyle, IntoElement, MouseButton, Window, WindowControlArea, div,
    prelude::*, px, svg,
};
use gpui_kit::base::Button as BaseButton;
use gpui_kit::{
    assets::IconName,
    component::{Icon, Sizable, Size},
};

use crate::{
    MainMode, RemoteBranchOperationKind, RepositoryView, WINDOW_CONTROLS_WIDTH,
    ui::{
        components::{control_shadow, icon_command_button},
        icons::{ToolbarIcon, toolbar_icon, toolbar_icon_with_size},
        theme::{self, rgb, rgba},
    },
};

/// 原生控制区固定在标题栏最右侧，窗口不能缩到把它们挤出视口。
pub(crate) const MIN_WINDOW_WIDTH: f32 = 860.0;
pub(crate) const MIN_WINDOW_HEIGHT: f32 = 520.0;
/// 底部状态栏保持单行高度，避免占用工作区。
pub(crate) const STATUS_BAR_HEIGHT: f32 = 18.0;
pub(crate) const NARROW_LAYOUT_WIDTH: f32 = 1120.0;
pub(crate) const COMFORTABLE_LAYOUT_WIDTH: f32 = 1440.0;
/// 悬浮工作台的内容区留白：四周与面板间隙同值（画板 16px）。
pub(crate) const SHELL_PADDING: f32 = theme::SPACE_4;

/// 圆角窗口的边框处理（Windows）。
///
/// 采用无边框/自绘标题栏窗口的行业标准做法（Electron / tao / winit / egui 同款）：
/// **一个样式位都不动**——gpui 创建时的 `WS_THICKFRAME | WS_MAXIMIZEBOX` 与系统
/// 补上的 `WS_CAPTION` 全部保留。系统的缩放循环、`WM_NCHITTEST` 边框命中区、
/// Aero Snap、双击顶边最大化都依赖这套样式（实测：只去 `WS_CAPTION` 后
/// `DefWindowProc` 的四边命中退回 `HTCLIENT`，缩放带失效；ZCode 的可缩放无边框
/// 窗口同样是 caption + thickframe 全保留）。
///
/// 自绘圆角外观不靠去样式实现，靠 `shell_frame` 子类化把客户区铺满窗口矩形：
/// 1. `WM_NCCALCSIZE`：非最大化时把客户区 left/right/bottom 恢复成 proposed
///    窗口矩形（gpui 只恢复了 top）。客户区与窗口等大，非客户区面积为 0，
///    DWM 没有边框可画，M1 记录的「顶部白线 / 客户区内缩 14 × 7」随之消失
///    （白线的来源是内缩出的非客户区环带，不是 caption 样式位本身）。
///    最大化时保持系统内缩，内容才不会超出屏幕。
/// 2. `WM_NCLBUTTONDOWN` / `WM_NCLBUTTONDBLCLK`：命中码落在边框区（`HTLEFT` /
///    `HTTOP` / 两个上角 / 底边及底角）时直接放行 `DefWindowProc`。gpui 会把
///    非客户区按下派发给元素树，被任何控件消费（`stop_propagation`）后
///    `DefWindowProc` 就收不到，系统缩放循环起不来；放行后缩放必然走系统的
///    modal loop（最小尺寸、DPI、贴边都由系统负责），弹窗遮罩打开时四边
///    同样可缩放。
///
/// 顶边与两个上角由 gpui 自己的命中测试按 DPI 给出（`handle_hit_test_msg`），
/// 与系统边框命中区重叠时以 gpui 优先。
///
/// 窗口创建后调用一次、下一帧再调用一次。
#[cfg(windows)]
pub(crate) fn apply_window_chrome(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_SETWORKAREA, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
        SetWindowPos, WM_SETTINGCHANGE,
    };

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    unsafe {
        // 装上子类化后重算一次非客户区：CreateWindow 时的 WM_NCCALCSIZE 发生在
        // 子类化安装之前，客户区还是内缩的，FRAMECHANGED 让窗口按新逻辑重算。
        shell_frame::install(hwnd);
        let _ = SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
        // gpui 的 border_offset 是窗口创建、客户区内缩时记下的 16 × 8，它参与
        // 尺寸换算（最小窗 860 × 520 会变成 876 × 528）。border_offset 按
        // 「窗口矩形 − 客户区」实测，客户区铺满后应为 0；发一条 WM_SETTINGCHANGE
        // 让 gpui 在这个时机重算（gpui 对非滚轮类 action 是空操作，无副作用）。
        SendMessageW(hwnd, WM_SETTINGCHANGE, SPI_SETWORKAREA as usize, 0);
    }
}

#[cfg(not(windows))]
pub(crate) fn apply_window_chrome(_window: &Window) {}

/// 窗口边框子类化（Windows）：自绘圆角窗口保留系统样式的三个补偿。
///
/// gpui 的窗口过程把所有消息据为己有（`handle_msg` 未处理才转 DefWindowProc），
/// 且把非客户区鼠标消息也派发给元素树，所以必须在它的 WndProc 之外包一层：
/// - `WM_NCCALCSIZE`：非最大化时把客户区 left/right/bottom 恢复成 proposed
///   窗口矩形（gpui 只恢复了 top），客户区与窗口等大，非客户区环带消失；
/// - `WM_NCHITTEST`：客户区铺满后 DefWindowProc 不再给边框命中（实测左右下
///   均返回 HTCLIENT），按系统 frame 厚度补左/右/下三边与两个下角的命中码
///   （顶边与两个上角 gpui 已按 DPI 自行给出）；
/// - `WM_NCLBUTTONDOWN` / `WM_NCLBUTTONDBLCLK`：命中码是边框区时直接放行
///   DefWindowProc，系统 modal 缩放循环必然启动，不受元素树消费影响。
#[cfg(windows)]
mod shell_frame {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, DefWindowProcW, GWLP_WNDPROC, GetWindowRect, IsZoomed, NCCALCSIZE_PARAMS,
        SetWindowLongPtrW, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, WM_NCCALCSIZE, WM_NCDESTROY,
        WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WNDPROC,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT};

    /// 边框命中码区间（`WinUser.h`）：`HTLEFT = 10` … `HTBOTTOMRIGHT = 17`。
    /// 覆盖左/右/上/下四边与四个角（`HTCAPTION`、`HTMIN/MAX/CLOSE` 等不在内）。
    const FRAME_HIT_FIRST: u32 = 10;
    const FRAME_HIT_LAST: u32 = 17;

    thread_local! {
        /// hwnd → 被替换掉的 gpui 窗口过程。同窗口只装一次；销毁时移除。
        static PREV_PROCS: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    }

    /// 安装子类化；重复调用是空操作（`apply_window_chrome` 每帧都会调到）。
    pub(crate) unsafe fn install(hwnd: HWND) {
        let key = hwnd as isize;
        let installed = PREV_PROCS.with(|procs| procs.borrow().contains_key(&key));
        if installed {
            return;
        }
        // 把 gpui 的窗口过程换成我们的子类过程，原过程登记后继续转发。
        let subclass: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
            shell_subclass_proc;
        let prev = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass as usize as isize) };
        if prev == 0 {
            return;
        }
        PREV_PROCS.with(|procs| {
            procs.borrow_mut().insert(key, prev);
        });
    }

    /// 取登记的原窗口过程；WNDPROC 本身就是 Option<fn>，未登记返回 None。
    fn prev_wnd_proc(hwnd: HWND) -> WNDPROC {
        PREV_PROCS
            .with(|procs| procs.borrow().get(&(hwnd as isize)).copied())
            .and_then(|prev| unsafe { std::mem::transmute::<isize, WNDPROC>(prev) })
    }

    fn is_frame_hit(hit: u32) -> bool {
        (FRAME_HIT_FIRST..=FRAME_HIT_LAST).contains(&hit)
    }

    /// 系统边框带的物理像素宽（左右用 x、上下用 y，均含 padded border）。
    fn frame_thickness(hwnd: HWND) -> (i32, i32) {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let x = unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) }
            + unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
        let y = unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) }
            + unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
        (x, y)
    }

    /// 光标（lparam 是屏幕坐标）落在哪条系统缩放边上；不在边框带内返回 0。
    ///
    /// 顶边与两个上角不在这里判：gpui 的 `handle_hit_test_msg` 已按 DPI 给出
    /// HTTOP/HTTOPLEFT/HTTOPRIGHT，且它的判定先于我们执行。
    fn frame_hit_test(hwnd: HWND, lparam: LPARAM) -> isize {
        let x = (lparam & 0xFFFF) as i16 as i32;
        let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
        let mut rect = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let ok = unsafe { GetWindowRect(hwnd, &mut rect) };
        if ok == 0 {
            return 0;
        }
        let (frame_x, frame_y) = frame_thickness(hwnd);
        let on_left = x - rect.left < frame_x;
        let on_right = rect.right - x < frame_x;
        let on_bottom = rect.bottom - y < frame_y;
        if on_left && on_bottom {
            HTBOTTOMLEFT as isize
        } else if on_right && on_bottom {
            HTBOTTOMRIGHT as isize
        } else if on_left {
            HTLEFT as isize
        } else if on_right {
            HTRIGHT as isize
        } else if on_bottom {
            HTBOTTOM as isize
        } else {
            0
        }
    }

    unsafe extern "system" fn shell_subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            let Some(prev) = prev_wnd_proc(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            match msg {
                // wparam != 0 时 lparam 指向 NCCALCSIZE_PARAMS，rgrc[0] 是 proposed
                // 窗口矩形（进入默认处理前的值）。转发给 gpui 后它已把 rgrc[0] 改成
                // 内缩后的客户区，这里用转发前存的 proposed 矩形把三边恢复回去。
                // 最大化时保持系统内缩：最大化窗口矩形含被屏幕裁掉的边框外扩，
                // 客户区若铺满会把内容顶出屏幕（gpui 对 top 也是同样处理）。
                WM_NCCALCSIZE if wparam != 0 && IsZoomed(hwnd) == 0 => {
                    let params = lparam as *mut NCCALCSIZE_PARAMS;
                    let proposed = (*params).rgrc[0];
                    let result = CallWindowProcW(Some(prev), hwnd, msg, wparam, lparam);
                    (*params).rgrc[0].left = proposed.left;
                    (*params).rgrc[0].right = proposed.right;
                    (*params).rgrc[0].bottom = proposed.bottom;
                    result
                }
                // 客户区铺满后 DefWindowProc 不再给边框命中（实测左右下均
                // HTCLIENT），gpui 自己只补顶边与两个上角。这里补左/右/下三边
                // 与两个下角：gpui 或 DefWindowProc 已给出的判定（HTTOP 系列、
                // HTCAPTION、窗口按钮）原样透传，只在结果是 HTCLIENT（=1）或
                // 空结果时按边框带重算。
                WM_NCHITTEST if IsZoomed(hwnd) == 0 => {
                    let forwarded = CallWindowProcW(Some(prev), hwnd, msg, wparam, lparam);
                    let forwarded_hit = forwarded;
                    if forwarded_hit > 1 {
                        return forwarded;
                    }
                    let hit = frame_hit_test(hwnd, lparam);
                    if hit != 0 {
                        hit
                    } else {
                        forwarded
                    }
                }
                // 边框按下/双击不走 gpui 的元素树：否则任何消费按下
                // （stop_propagation）的控件都会让 DefWindowProc 收不到消息，
                // 系统缩放循环起不来。顶角双击的放大/还原语义由系统 loop 负责。
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if is_frame_hit(wparam as u32) => {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_NCDESTROY => {
                    let result = CallWindowProcW(Some(prev), hwnd, msg, wparam, lparam);
                    // 窗口即将销毁：卸掉子类化并清理登记，避免遗留指向已析构窗口的记录。
                    let restore = std::mem::transmute::<WNDPROC, isize>(Some(prev));
                    SetWindowLongPtrW(hwnd, GWLP_WNDPROC, restore);
                    PREV_PROCS.with(|procs| {
                        procs.borrow_mut().remove(&(hwnd as isize));
                    });
                    result
                }
                _ => CallWindowProcW(Some(prev), hwnd, msg, wparam, lparam),
            }
        }
    }
}

/// 根壳中间区使用确定高度，避免页面最小高度把状态栏和导航器底部推出视口。
///
/// 中间区的固定高度已包含上下内边距，不再重复扣除留白。
pub(crate) fn shell_content_height(viewport_height: f32) -> f32 {
    (viewport_height - theme::TITLEBAR_HEIGHT - STATUS_BAR_HEIGHT).max(0.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutBand {
    Narrow,
    Standard,
    Comfortable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShellLayoutPolicy {
    pub(crate) band: LayoutBand,
    pub(crate) show_context_navigator: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextNavigatorPresentation {
    Hidden,
    Docked,
    Overlay,
}

/// 宽度策略只描述信息优先级：窄窗口覆盖 Context Navigator，
/// 绝不挤压 titlebar 的原生命中区域；窄档下顶栏命令统一收成纯图标。
pub(crate) const fn shell_layout_policy(width: f32) -> ShellLayoutPolicy {
    if width < NARROW_LAYOUT_WIDTH {
        ShellLayoutPolicy {
            band: LayoutBand::Narrow,
            show_context_navigator: false,
        }
    } else if width < COMFORTABLE_LAYOUT_WIDTH {
        ShellLayoutPolicy {
            band: LayoutBand::Standard,
            show_context_navigator: true,
        }
    } else {
        ShellLayoutPolicy {
            band: LayoutBand::Comfortable,
            show_context_navigator: true,
        }
    }
}

/// 模式的中文名（收起窄条的图标按钮用 tooltip 表达；展开态按钮直接显示文字）。
fn navigator_mode_label(mode: MainMode) -> &'static str {
    match mode {
        MainMode::Worktree => "工作区",
        MainMode::Conflict => "冲突处理",
        MainMode::History => "提交记录",
        MainMode::Workflow => "工作流",
        MainMode::Stash => "贮藏",
        MainMode::Browse => "分支浏览",
        MainMode::Blame => "追溯",
        MainMode::CommitGraph => "提交图谱",
    }
}

/// 顶栏命令组：药丸按钮之间 10px，与仓库切换器之间 12px。
fn chrome_command_group() -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.0))
        .ml(px(theme::SPACE_3))
}

/// 为壳层命令提供可操作的禁用说明，避免 disabled 控件只有灰色视觉却没有原因。
fn chrome_action_disabled_reason(
    label: &'static str,
    repo_open: bool,
    remote_open: bool,
    busy: bool,
    merge_in_progress: bool,
) -> Option<&'static str> {
    if busy {
        return Some("当前操作进行中，请稍候");
    }
    if !repo_open {
        return Some("请先打开仓库");
    }

    if matches!(label, "获取" | "拉取" | "推送") && !remote_open {
        return Some("当前仓库没有可用远端");
    }
    if matches!(label, "拉取" | "推送") && merge_in_progress {
        return Some("合并进行中，完成或中止合并后再操作");
    }
    None
}

/// 只有主工作台页面承载仓库上下文；专用模式保留完整画布，不显示无意义的展开入口。
pub(crate) const fn context_navigator_supported_mode(mode: MainMode) -> bool {
    matches!(
        mode,
        MainMode::Worktree | MainMode::History | MainMode::Workflow
    )
}

/// Navigator 的呈现由真实窗口宽度、模式偏好和窄窗临时覆盖态共同决定。
/// `Hidden` 表示收起为窄条（展开箭头 + 模式图标），并非消失。
pub(crate) const fn context_navigator_presentation(
    policy: ShellLayoutPolicy,
    mode: MainMode,
    dock_requested: bool,
    overlay_requested: bool,
) -> ContextNavigatorPresentation {
    if !context_navigator_supported_mode(mode) {
        ContextNavigatorPresentation::Hidden
    } else if policy.show_context_navigator && dock_requested {
        ContextNavigatorPresentation::Docked
    } else if !policy.show_context_navigator && overlay_requested {
        ContextNavigatorPresentation::Overlay
    } else {
        ContextNavigatorPresentation::Hidden
    }
}

impl RepositoryView {
    pub(crate) fn shell_layout_policy(&self, window: &Window) -> ShellLayoutPolicy {
        shell_layout_policy(window.viewport_size().width.into())
    }

    pub(crate) fn context_navigator_presentation(
        &self,
        window: &Window,
    ) -> ContextNavigatorPresentation {
        context_navigator_presentation(
            self.shell_layout_policy(window),
            self.main_mode,
            self.context_navigator_preferences
                .is_visible(self.main_mode),
            self.context_navigator_overlay_open,
        )
    }

    pub(crate) fn render_chrome_titlebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let policy = self.shell_layout_policy(window);
        // 窄档只留图标 + tooltip：顶栏 60px 里放得下全部命令，但不放长文案。
        let compact = policy.band == LayoutBand::Narrow;
        let repo_open = self.repo_path.is_some();
        let remote_open = !self.loading.remote() && self.current_remote().is_some();
        let merge_in_progress = self.merge_in_progress();
        let behind_count = self
            .branch_sync_status
            .as_ref()
            .map(|status| status.behind)
            .unwrap_or(0);
        let ahead_count = self
            .branch_sync_status
            .as_ref()
            .map(|status| status.ahead)
            .unwrap_or(0);

        div()
            .id("focus-workbench-titlebar")
            .flex()
            .flex_none()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .h(px(theme::TITLEBAR_HEIGHT))
            // 画板：品牌左边距 24、窗口按钮右边距 12；顶栏不画底边线，
            // 工具栏底色与环境底只差一档明度，靠留白分层。
            .pl(px(theme::SPACE_6))
            .pr(px(theme::WINDOW_CONTROLS_RIGHT_INSET))
            .bg(rgb(theme::WB_TOOLBAR))
            // 顶栏是唯一铺满窗口宽度的非环境色块：上角必须与外壳圆角一致，
            // 否则两角会把圆角糊成直角（gpui 的 overflow 只裁矩形，裁不住圆角）。
            .rounded_tl(px(theme::window_radius()))
            .rounded_tr(px(theme::window_radius()))
            .child(self.render_chrome_brand(compact))
            .child(self.render_repo_switcher_button(cx))
            .child(
                chrome_command_group()
                    .child(self.chrome_command_button(
                        "刷新",
                        IconName::RefreshCw,
                        None,
                        repo_open && !self.busy,
                        compact,
                        |this, _window, _cx| this.refresh(),
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "获取",
                        IconName::CloudDownload,
                        None,
                        repo_open && remote_open && !self.busy,
                        compact,
                        |this, _window, _cx| this.fetch(),
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "拉取",
                        IconName::ArrowDownToLine,
                        (behind_count > 0).then(|| format!("↓{behind_count}")),
                        repo_open && remote_open && !self.busy && !merge_in_progress,
                        compact,
                        |this, _window, _cx| {
                            this.open_remote_branch_operation(RemoteBranchOperationKind::Pull)
                        },
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "推送",
                        IconName::ArrowUpFromLine,
                        (ahead_count > 0).then(|| format!("↑{ahead_count}")),
                        repo_open && remote_open && !self.busy && !merge_in_progress,
                        compact,
                        |this, _window, _cx| {
                            this.open_remote_branch_operation(RemoteBranchOperationKind::Push)
                        },
                        cx,
                    ))
                    // 「贮藏」「子模块」与刷新/获取/拉取/推送同一策略：宽窗显示
                    // 图标 + 文字，窄档（< 1120）自动收成纯图标，顶栏宽度可控。
                    .child(self.chrome_command_button(
                        "贮藏",
                        IconName::Archive,
                        None,
                        repo_open && !self.busy && !merge_in_progress,
                        compact,
                        |this, _window, _cx| this.open_stash_dialog(),
                        cx,
                    ))
                    .child(self.chrome_command_button(
                        "子模块",
                        IconName::Boxes,
                        None,
                        repo_open && !self.busy,
                        compact,
                        |this, _window, _cx| this.open_submodule_manager(),
                        cx,
                    )),
            )
            .child(self.render_chrome_search_entry(compact, cx))
            .child(self.render_chrome_drag_area())
            .child(self.render_chrome_window_controls(window))
    }

    /// 顶栏全局搜索入口：外观与输入框一致，点击打开 Ctrl+P 符号检索面板
    /// （真正的输入框在面板里，避免顶栏再造一份文本状态）。
    fn render_chrome_search_entry(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = !self.busy;
        BaseButton::new("chrome-search-entry")
            .disabled(!enabled)
            .accessibility_label("搜索文件、符号或命令")
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .flex_1()
            .min_w(px(if compact { 96.0 } else { 140.0 }))
            .max_w(px(490.0))
            .ml(px(theme::SPACE_4))
            .h(px(theme::CONTROL_HEIGHT_TOOLBAR))
            .px(px(theme::SPACE_3))
            .flex()
            .items_center()
            .gap(px(theme::SPACE_3))
            .rounded(px(theme::RADIUS_MD))
            .bg(rgb(theme::WB_INPUT_SURFACE))
            .when(enabled, |this| this.cursor_pointer())
            .when(!enabled, |this| this.cursor_not_allowed().opacity(0.6))
            .child(toolbar_icon(ToolbarIcon::Search, theme::CONTENT_TERTIARY))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(theme::TYPE_BODY))
                    .text_color(rgb(theme::CONTENT_TERTIARY))
                    .child("搜索文件、符号或命令"),
            )
            .when(!compact, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(px(theme::TYPE_META))
                        .text_color(rgb(theme::CONTENT_TERTIARY))
                        .child("Ctrl P"),
                )
            })
            .on_click(cx.listener(move |this, _event, window, cx| {
                if !enabled {
                    return;
                }
                // 与 Ctrl+P 同一入口：面板打开时再次点击关闭。
                this.toggle_code_search_palette(window, cx);
            }))
    }

    /// Context Navigator 的模式按钮条目（收起窄条图标与展开态文字按钮共用同一来源，
    /// 保证两态顺序一致）。设置入口在 titlebar，不在此列；有冲突时在「工作区」后
    /// 追加「冲突处理」条件条目。
    fn navigator_mode_entries(
        &self,
    ) -> Vec<(&'static str, ToolbarIcon, &'static str, bool, MainMode)> {
        let has_conflicts = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.conflicts.is_empty());
        let mut entries = vec![(
            "nav-worktree",
            ToolbarIcon::Worktree,
            "工作区",
            self.main_mode == MainMode::Worktree,
            MainMode::Worktree,
        )];
        if has_conflicts {
            entries.push((
                "nav-conflict",
                ToolbarIcon::Ai,
                "冲突处理",
                self.main_mode == MainMode::Conflict,
                MainMode::Conflict,
            ));
        }
        entries.push((
            "nav-history",
            ToolbarIcon::History,
            "提交记录",
            self.main_mode == MainMode::History,
            MainMode::History,
        ));
        entries.push((
            "nav-workflow",
            ToolbarIcon::Workflow,
            "工作流",
            self.main_mode == MainMode::Workflow,
            MainMode::Workflow,
        ));
        entries
    }

    /// Context Navigator 收起态窄条：全高 48px 圆角面板，顶部展开箭头 + 下方模式图标
    /// （32px 方块、图标中心 x=24），底部为设置入口。专用页面（冲突/贮藏/浏览/追溯）
    /// 不显示分组列表，但窄条仍常驻——模式图标是这些页面返回工作台/历史的唯一入口。
    pub(crate) fn render_navigator_collapsed_strip(
        &self,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 专用页面没有可展开的导航区，箭头禁用并说明原因（窄窗覆盖逻辑同样不可达）。
        let toggle_enabled = context_navigator_supported_mode(self.main_mode);
        let strip = crate::ui::components::navigator_panel()
            .id("navigator-collapsed-strip")
            .flex()
            .flex_none()
            .flex_col()
            .items_center()
            .h_full()
            .min_h(px(0.0))
            .w(px(theme::NAVIGATOR_COLLAPSED_WIDTH))
            // 面板之间的间隙由拖拽区提供；窄条收起时没有拖拽区，用右边距留出同样的间隙。
            .mr(px(SHELL_PADDING))
            .py(px(theme::SPACE_2))
            .child(self.render_context_navigator_toggle(overlay, toggle_enabled, cx))
            // 箭头与模式图标之间留出间隔，避免两排图标贴在一起
            .child(div().h(px(theme::SPACE_2)));
        self.navigator_mode_entries()
            .into_iter()
            .fold(strip, |strip, (id, icon, _label, active, mode)| {
                strip.child(self.navigator_mode_button(id, icon, active, mode, cx))
            })
            // 底部：设置入口常驻（展开/收起两态都在同一位置）。
            .child(div().flex_1())
            .child(self.navigator_settings_button("navigator-settings-strip", cx))
    }

    /// 展开态模式按钮：图标与文字是**同一个**按钮--悬停、按下、选中反馈整行同步。
    /// 左内边距让图标中心落在 x=24，与收起窄条图标位置一致（两态切换图标零位移）。
    ///
    /// `justify_start` 必须显式声明：Kit `Button` 自带 `justify_center`，整行按钮若
    /// 不覆盖它，图标 + 文字会缩在行中部，与「左对齐的功能区」设计不符。
    fn navigator_expanded_mode_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        label: &'static str,
        active: bool,
        mode: MainMode,
        cx: &mut Context<Self>,
    ) -> BaseButton {
        BaseButton::new(format!("navigator-mode-{id}"))
            .selected(active)
            .accessibility_label(label)
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .relative()
            .flex_none()
            .w_full()
            .h(px(theme::CONTROL_HEIGHT_REGULAR + 4.0))
            .mb(px(theme::SPACE_1))
            .flex()
            .items_center()
            .justify_start()
            // 图标槽 16px：pl(16) + 槽中心 8 -> 图标中心 x=24（与收起窄条一致）
            .pl(px(16.0))
            .gap(px(theme::SPACE_2))
            .rounded(px(theme::RADIUS_MD))
            .cursor_pointer()
            .text_size(px(theme::TYPE_TITLE))
            .bg(if active {
                rgb(theme::PRIMARY_SUBTLE)
            } else {
                rgba(0x00000000)
            })
            .text_color(rgb(if active {
                theme::PRIMARY
            } else {
                theme::CONTENT_SECONDARY
            }))
            .hover(|this| this.bg(rgb(theme::WB_ROW_HOVER)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.set_main_mode(mode);
                cx.notify();
            }))
            .child(toolbar_icon(
                icon,
                if active {
                    theme::PRIMARY
                } else {
                    theme::CONTENT_SECONDARY
                },
            ))
            .child(label)
    }

    /// `enabled` 为 false 时按钮禁用（专用页面没有可展开的导航区）。
    pub(crate) fn render_context_navigator_toggle(
        &self,
        overlay: bool,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let visible = if overlay {
            self.context_navigator_overlay_open
        } else {
            self.context_navigator_preferences
                .is_visible(self.main_mode)
        };
        // 箭头表达点击后的移动方向：展开时指左（收起导航），收起时指右（展开导航）。
        let toggle_icon = if visible {
            ToolbarIcon::ChevronLeft
        } else {
            ToolbarIcon::ChevronRight
        };
        icon_command_button(
            "context-navigator-toggle".into(),
            toggle_icon,
            if visible {
                "收起上下文导航"
            } else {
                "展开上下文导航"
            },
            enabled,
            move |this, _window, _cx| {
                if overlay {
                    this.context_navigator_overlay_open = !this.context_navigator_overlay_open;
                } else {
                    let mode = this.main_mode;
                    this.context_navigator_preferences.toggle(mode);
                    // 展开/收起是持久偏好，切换后立即落库（重启恢复）。
                    this.save_layout_preferences();
                }
            },
            cx,
        )
        .when(visible, |this| this.bg(rgb(theme::PRIMARY_SUBTLE)))
    }

    pub(crate) fn render_context_navigator(
        &self,
        window: &Window,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 模式按钮区：导航面板顶部，图标 + 文字整行按钮，点击切换主页面。
        // 画板没有「上下文导航」标题行：标题省去，收起入口移到底部与设置同排。
        let mode_buttons = self.navigator_mode_entries().into_iter().fold(
            div()
                .id("navigator-mode-buttons")
                .flex()
                .flex_none()
                .flex_col()
                .p(px(theme::SPACE_2)),
            |buttons, (id, icon, label, active, mode)| {
                buttons
                    .child(self.navigator_expanded_mode_button(id, icon, label, active, mode, cx))
            },
        );
        crate::ui::components::navigator_panel()
            .id("context-navigator")
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.sidebar_width))
            .h_full()
            .min_h(px(0.0))
            .child(mode_buttons)
            .child(self.render_sidebar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(px(theme::SPACE_2))
                    .pb(px(theme::SPACE_2))
                    .child(self.navigator_settings_button("navigator-settings", cx))
                    .child(self.render_context_navigator_toggle(overlay, true, cx)),
            )
    }

    pub(crate) fn render_context_navigator_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .top(px(0.0))
            // 覆盖层左缘贴收起窄条右缘（窄条在内容区留白之后，恒为最左列）。
            .left(px(SHELL_PADDING + theme::NAVIGATOR_COLLAPSED_WIDTH))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .bg(crate::ui::theme::rgba(theme::DIALOG_OVERLAY))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.context_navigator_overlay_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_none()
                    .h_full()
                    // 原生 GPUI 仅支持 tab group，不伪造临时焦点句柄；将 Tab 顺序限制在导航区。
                    .tab_group()
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                        cx.stop_propagation();
                    })
                    .child(self.render_context_navigator(window, true, cx)),
            )
    }

    /// 设置入口（导航面板底部，展开与收起两态共用）：36 × 36 齿轮图标，
    /// 无底色、悬停才出薄底，点击打开设置中心并聚焦（Ctrl+, 等效）。
    fn navigator_settings_button(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        BaseButton::new(id)
            .accessibility_label("设置")
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .flex_none()
            .size(px(theme::CONTROL_HEIGHT_TOOLBAR))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(theme::WB_ROW_HOVER)))
            .tooltip(|_window, cx| crate::ui::components::tooltip_text("设置", cx))
            .on_click(cx.listener(|this, _event, window, cx| {
                this.open_settings_center();
                window.focus(&this.settings_center_focus, cx);
                cx.notify();
            }))
            .child(
                Icon::new(IconName::Settings)
                    .with_size(Size::Size(px(18.0)))
                    .text_color(rgb(theme::CONTENT_SECONDARY)),
            )
    }

    /// 品牌区：应用标志 + 字标。窄档只留标志，把宽度让给命令与搜索。
    /// 整块同时是窗口拖拽区（拖拽/双击最大化仍由系统 caption 语义承担）。
    fn render_chrome_brand(&self, compact: bool) -> impl IntoElement {
        div()
            .id("titlebar-brand")
            .flex()
            .flex_none()
            .h_full()
            .items_center()
            .gap(px(theme::SPACE_2))
            .cursor(CursorStyle::Arrow)
            .window_control_area(WindowControlArea::Drag)
            .child(
                // v6「负空间分支」应用标志（assets/icons/app-mark.svg）。
                // svg() 按渲染 alpha 染色：实心方块取主题色 PRIMARY（跟随主题色切换），
                // 刻痕透出顶栏底色，与桌面 app.ico 同构。
                svg()
                    .path("icons/app-mark.svg")
                    .size(px(26.0))
                    .text_color(rgb(theme::PRIMARY))
                    .flex_none(),
            )
            .when(!compact, |this| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_BRAND))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(theme::CONTENT_PRIMARY))
                        .child("Khaslana"),
                )
            })
    }

    fn render_chrome_drag_area(&self) -> impl IntoElement {
        div()
            .id("titlebar-drag-area")
            .flex_1()
            .min_w(px(24.0))
            .h_full()
            .window_control_area(WindowControlArea::Drag)
    }

    /// 窗口控制：32 × 32、间距 2px、距右缘 12px（画板第五版）。
    ///
    /// 最大化走 `WindowControlArea::Max` 原生命中区——gpui-pre-windows 对
    /// `HTMAXBUTTON` 的单击本身就是最大化/还原切换，所以这里只需按
    /// `is_maximized()` 换图标；不要改用 `zoom_window()`（它只最大化，不会还原）。
    fn render_chrome_window_controls(&self, window: &Window) -> gpui::Div {
        let maximize_icon = if window.is_maximized() {
            ToolbarIcon::Restore
        } else {
            ToolbarIcon::Maximize
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(theme::WINDOW_CONTROL_GAP))
            .w(px(WINDOW_CONTROLS_WIDTH))
            .h_full()
            .child(self.chrome_window_control_button(
                "window-minimize",
                ToolbarIcon::Minus,
                false,
                WindowControlArea::Min,
            ))
            .child(self.chrome_window_control_button(
                "window-maximize",
                maximize_icon,
                false,
                WindowControlArea::Max,
            ))
            .child(self.chrome_window_control_button(
                "window-close",
                ToolbarIcon::Close,
                true,
                WindowControlArea::Close,
            ))
    }

    fn chrome_window_control_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        danger: bool,
        area: WindowControlArea,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_none()
            .size(px(theme::WINDOW_CONTROL_SIZE))
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_XS))
            .cursor_pointer()
            .window_control_area(area)
            .hover(move |this| {
                this.bg(rgb(if danger {
                    theme::FEEDBACK_ERROR_BG
                } else {
                    theme::WB_ROW_HOVER
                }))
            })
            // 关闭按钮悬停时图标转危险色，其余保持中性（画板：仅关闭有红反馈）。
            .text_color(rgb(if danger {
                theme::FEEDBACK_ERROR_TEXT
            } else {
                theme::CONTENT_SECONDARY
            }))
            .child(toolbar_icon_with_size(
                icon,
                if danger {
                    theme::FEEDBACK_ERROR_TEXT
                } else {
                    theme::CONTENT_SECONDARY
                },
                14.0,
                14.0,
            ))
    }

    fn chrome_command_button(
        &self,
        label: &'static str,
        icon_kind: IconName,
        sync_label: Option<String>,
        enabled: bool,
        compact: bool,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> BaseButton {
        let disabled_reason = if enabled {
            None
        } else {
            chrome_action_disabled_reason(
                label,
                self.repo_path.is_some(),
                !self.loading.remote() && self.current_remote().is_some(),
                self.busy,
                self.merge_in_progress(),
            )
        };
        // 图标按钮（窄档与「贮藏/子模块」）靠 tooltip 传达命令名；
        // 禁用时优先说明原因，避免只有灰掉的图标。
        let tooltip_label = disabled_reason.or(compact.then_some(label));
        BaseButton::new(format!("chrome-command-{label}"))
            .disabled(!enabled)
            .accessibility_label(label)
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(theme::SPACE_2))
            .h(px(theme::CONTROL_HEIGHT_TOOLBAR))
            .when(compact, |this| {
                this.w(px(theme::CONTROL_HEIGHT_TOOLBAR)).justify_center()
            })
            .when(!compact, |this| this.px(px(theme::SPACE_3)))
            .rounded(px(theme::RADIUS_MD))
            // 顶栏命令是「白色薄实体 + 接触阴影」：与工具栏底色拉开一档，而不是描边。
            .bg(rgb(theme::WB_PANEL))
            .shadow(control_shadow())
            .text_color(rgb(if enabled {
                theme::CONTENT_PRIMARY
            } else {
                theme::CONTENT_TERTIARY
            }))
            .text_size(px(theme::TYPE_BODY))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(theme::WB_ROW_HOVER)))
                    .active(|this| this.opacity(0.8))
            })
            .when(!enabled, |this| this.cursor_not_allowed().opacity(0.5))
            .when_some(tooltip_label, |this, text| {
                this.tooltip(move |_window, cx| crate::ui::components::tooltip_text(text, cx))
            })
            .on_click(cx.listener(move |this, _event, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                    cx.notify();
                }
            }))
            .child(
                Icon::new(icon_kind)
                    .with_size(Size::Size(px(16.0)))
                    .text_color(rgb(if enabled {
                        theme::CONTENT_SECONDARY
                    } else {
                        theme::CONTENT_TERTIARY
                    })),
            )
            .when(!compact, |this| this.child(label))
            .when_some(sync_label, |this, label| {
                this.child(
                    div()
                        .text_size(px(theme::TYPE_META))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(theme::PRIMARY))
                        .child(label),
                )
            })
    }

    /// 收起窄条的模式图标按钮：32px 方块，支持 Tab 与 Enter/Space。
    fn navigator_mode_button(
        &self,
        id: &'static str,
        icon: ToolbarIcon,
        active: bool,
        mode: MainMode,
        cx: &mut Context<Self>,
    ) -> BaseButton {
        BaseButton::new(id)
            .selected(active)
            .accessibility_label(navigator_mode_label(mode))
            .focus_visible(|this| this.border_1().border_color(rgb(theme::PRIMARY)))
            .relative()
            .flex_none()
            .size(px(theme::CONTROL_HEIGHT_REGULAR + 4.0))
            .mb(px(theme::SPACE_1))
            .rounded(px(theme::RADIUS_MD))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(if active {
                rgb(theme::PRIMARY_SUBTLE)
            } else {
                rgba(0x00000000)
            })
            .text_color(rgb(if active {
                theme::PRIMARY
            } else {
                theme::CONTENT_SECONDARY
            }))
            .hover(|this| this.bg(rgb(theme::WB_ROW_HOVER)))
            .tooltip(move |_window, cx| {
                crate::ui::components::tooltip_text(navigator_mode_label(mode), cx)
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.set_main_mode(mode);
                cx.notify();
            }))
            .child(toolbar_icon_with_size(
                icon,
                if active {
                    theme::PRIMARY
                } else {
                    theme::CONTENT_SECONDARY
                },
                18.0,
                18.0,
            ))
    }
}

#[cfg(test)]
#[path = "tests/chrome_view.rs"]
mod tests;
