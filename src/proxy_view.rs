use gpui::{Context, IntoElement, Window, div, prelude::*, px};
use khaslana::NetworkProxyMode;

use crate::ui::theme::rgb;
use crate::{
    FieldId, RepositoryView,
    ui::{
        components::{dialog_actions, segmented_button},
        theme as ui_theme,
    },
};

impl RepositoryView {
    pub(crate) fn render_network_proxy_settings_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let custom_enabled = self.proxy_mode == NetworkProxyMode::Custom;
        let remote_label = self
            .current_remote()
            .map(|remote| format!("测试将连接当前远端：{remote}"))
            .unwrap_or_else(|| "测试代理需要先打开带远端的仓库".to_string());

        self.dialog_panel("代理设置", cx)
            .w(px(620.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.proxy_mode_button("不使用代理", NetworkProxyMode::Disabled, cx))
                    .child(self.proxy_mode_button("使用系统代理", NetworkProxyMode::System, cx))
                    .child(self.proxy_mode_button("自定义代理", NetworkProxyMode::Custom, cx)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(proxy_mode_help(self.proxy_mode)),
            )
            .when(custom_enabled, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(self.input(FieldId::ProxyHttpUrl, false, window, cx))
                        .child(self.input(FieldId::ProxyHttpsUrl, false, window, cx))
                        .child(self.input(FieldId::ProxySocks5Url, false, window, cx))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                                .child("代理认证第一版请写在 URL 中，例如 http://user:pass@127.0.0.1:7890。"),
                        ),
                )
            })
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(ui_theme::BORDER))
                    .bg(rgb(ui_theme::CARD))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(ui_theme::MUTED_FOREGROUND))
                    .child(remote_label),
            )
            .child(
                dialog_actions()
                    .child(self.button("取消", !self.busy, |this, _, _| this.close_dialog(), cx))
                    .child(self.button(
                        "测试代理",
                        !self.busy,
                        |this, _, _| this.test_network_proxy_settings(),
                        cx,
                    ))
                    .child(self.primary_button(
                        "保存",
                        !self.busy,
                        |this, _, _| this.save_network_proxy_settings_and_close(),
                        cx,
                    )),
            )
    }

    fn proxy_mode_button(
        &self,
        label: &'static str,
        mode: NetworkProxyMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.proxy_mode == mode;
        segmented_button(format!("proxy-mode-{label}"), selected, !self.busy)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if !this.busy {
                    this.set_proxy_mode(mode);
                    cx.notify();
                }
            }))
            .child(label)
    }
}

fn proxy_mode_help(mode: NetworkProxyMode) -> &'static str {
    match mode {
        NetworkProxyMode::Disabled => "Git 网络操作将显式直连，不使用 Git 配置或环境变量代理。",
        NetworkProxyMode::System => {
            "使用 libgit2 自动代理：优先读取 Git 代理配置，其次读取 http_proxy / https_proxy 环境变量；不读取系统 UI 代理或 PAC。"
        }
        NetworkProxyMode::Custom => {
            "按远端协议选择自定义代理；HTTP/HTTPS 远端可回退 SOCKS5，SSH 远端仅尝试自定义 SOCKS5。"
        }
    }
}
