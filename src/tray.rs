use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuId, MenuItem},
};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow},
};

const SHOW_MENU_ID: &str = "khaslana-tray-show";
const EXIT_MENU_ID: &str = "khaslana-tray-exit";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrayAction {
    Show,
    Exit,
}

/// Windows 系统托盘及主窗口句柄的轻量封装。
///
/// 托盘图标必须在 GPUI 所在的 Win32 消息循环线程创建，因此由 `RepositoryView`
/// 在 UI 线程持有；事件则在已有的 UI tick 中轮询，避免额外启动消息循环。
pub(crate) struct TrayController {
    _icon: TrayIcon,
    show_menu_id: MenuId,
    exit_menu_id: MenuId,
    window_handle: Option<HWND>,
}

impl TrayController {
    pub(crate) fn new() -> Result<Self, String> {
        let show_item = MenuItem::with_id(SHOW_MENU_ID, "显示主窗口", true, None);
        let exit_item = MenuItem::with_id(EXIT_MENU_ID, "退出 Khaslana", true, None);
        let menu = Menu::with_items(&[&show_item, &exit_item])
            .map_err(|error| format!("创建托盘菜单失败：{error}"))?;
        // app.rc 将应用图标以资源编号 1 嵌入，可直接复用而无需运行时文件路径。
        let icon = Icon::from_resource(1, Some((32, 32)))
            .map_err(|error| format!("读取托盘图标失败：{error}"))?;
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("Khaslana")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| format!("创建系统托盘失败：{error}"))?;

        Ok(Self {
            _icon: tray_icon,
            show_menu_id: show_item.id().clone(),
            exit_menu_id: exit_item.id().clone(),
            window_handle: None,
        })
    }

    pub(crate) fn attach_window(&mut self, window: &Window) -> Result<(), String> {
        self.window_handle = Some(window_hwnd(window)?);
        Ok(())
    }

    pub(crate) fn hide_window(&mut self, window: &Window) -> Result<(), String> {
        let hwnd = window_hwnd(window)?;
        self.window_handle = Some(hwnd);
        // SAFETY：句柄来自当前仍存活的 GPUI 主窗口，调用发生在其 UI 线程。
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
        Ok(())
    }

    pub(crate) fn show_window(&self) {
        let Some(hwnd) = self.window_handle else {
            return;
        };
        // SAFETY：窗口只在应用退出时销毁；托盘控制器与主窗口由同一视图持有。
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }

    pub(crate) fn next_action(&self) -> Option<TrayAction> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.show_menu_id {
                return Some(TrayAction::Show);
            }
            if event.id == self.exit_menu_id {
                return Some(TrayAction::Exit);
            }
        }

        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
                | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => return Some(TrayAction::Show),
                _ => {}
            }
        }
        None
    }
}

fn window_hwnd(window: &Window) -> Result<HWND, String> {
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| format!("读取主窗口句柄失败：{error}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as HWND),
        _ => Err("当前窗口不是 Win32 窗口，无法缩小到系统托盘".to_string()),
    }
}
