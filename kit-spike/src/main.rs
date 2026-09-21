//! Khaslana 工作区原生样板（重构计划 M1 的隔离验证窗口）。
//!
//! - 只依赖 `gpui-kit`，跑在 Kit 自己的 GPUI 运行时上，不引用主工程的 `gpui-ce` 与 `yororen_ui`。
//! - 视觉对照 Pencil 画板 `Sgcwi` / `a4JtW` / `GuA1a`，数值见 `docs/gpui-kit-pencil-design.md`。
//! - 不连接真实仓库、凭据或网络，仅验证原生还原度与组件接缝。

mod theme;
mod tokens;
mod workbench;

use gpui_kit::component::Root;
use gpui_kit::*;

#[cfg(windows)]
fn hide_system_border(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
    };

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as _);
    let color = DWMWA_COLOR_NONE;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &color as *const _ as *const _,
            std::mem::size_of_val(&color) as u32,
        );
    }

    // 自绘圆角窗口不需要任何系统边框，两处都要去掉：
    // - WS_THICKFRAME：gpui-pre 为可缩放保留它，隐藏标题栏时 DefWindowProc 会把它变成
    //   持久的 1px 顶边；
    // - WS_CAPTION（= WS_BORDER | WS_DLGFRAME）：gpui 只传 WS_THICKFRAME，Windows 会为它
    //   补上 caption，它让窗口保留
    //   8px 的不可见缩放边框（客户区比窗口矩形小一圈），DWM 还会沿窗口顶边画 1px 边框。
    //   圆角外是透明区域，这条线正好从那里透出来，成为用户看到的「顶部白色横线」。
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SendMessageW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SPI_SETWORKAREA,
        SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_SETTINGCHANGE, WS_CAPTION,
        WS_THICKFRAME,
    };
    let raw_hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let style = GetWindowLongPtrW(raw_hwnd, GWL_STYLE);
        if style != 0 {
            let frame = WS_THICKFRAME as isize | WS_CAPTION as isize;
            SetWindowLongPtrW(raw_hwnd, GWL_STYLE, style & !frame);
            let _ = SetWindowPos(
                raw_hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
            // gpui-pre 只在 DPI 或系统设置变化时重算边框偏移，而它是在窗口创建、caption
            // 还在时就记下 16×8 的：去掉边框后偏移仍按旧值参与尺寸换算，窗口最小尺寸
            // 会被放大同样的量（860×520 只能到 876×528）。发一条设置变更消息触发重算；
            // gpui 对非滚轮类 action 是空操作，无副作用。
            SendMessageW(
                raw_hwnd,
                WM_SETTINGCHANGE,
                SPI_SETWORKAREA as usize,
                0,
            );
        }
    }
}

#[cfg(not(windows))]
fn hide_system_border(_window: &Window) {}

fn main() {
    // 图标资源必须显式注册：Kit 的 `init` 只初始化组件层，不注册 AssetSource。
    // 默认 `Assets` 只带 101 个组件图标，样板要用 Git/导航类图标，故取完整目录。
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            theme::apply(cx);

            // 对照画板：1440 × 1060；最小窗沿用产品约束 860 × 520。
            let bounds = Bounds::centered(None, size(px(1440.), px(1060.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    window_background: WindowBackgroundAppearance::Transparent,
                    window_min_size: Some(size(px(860.), px(520.))),
                    ..Default::default()
                },
                |window, cx| {
                    hide_system_border(window);
                    window.on_next_frame(|window, _cx| hide_system_border(window));
                    let view = cx.new(|cx| workbench::WorkbenchView::new(window, cx));
                    cx.new(|cx| {
                        Root::new(view, window, cx)
                            .bordered(false)
                            .bg(rgba(0x00000000))
                    })
                },
            )
            .expect("failed to open window");

            cx.activate(true);
        });
}
