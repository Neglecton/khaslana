use gpui::{Context, IntoElement, Window, div, prelude::*, px};
use khaslana::NetworkProxyMode;

use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};

use crate::ui::{components::tooltip_text, theme::rgb};
use crate::{
    FieldId, RepositoryView,
    ui::{
        components::{dialog_actions, segmented_button},
        theme as ui_theme,
    },
};

impl RepositoryView {
    /// 网络代理页的分组（Kit `SettingGroup` 列表）。
    ///
    /// 「自定义地址」只在自定义代理模式下存在，分组数量随模式变化：Kit 的
    /// `SettingsFilter::selected_index` 会自动清掉不存在分组的选中态，
    /// 侧栏子导航与内容区同步增减，不会出现悬空高亮。
    pub(crate) fn settings_proxy_groups(
        &self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<SettingGroup> {
        let view = cx.entity();
        let custom_enabled = self.proxy_mode == NetworkProxyMode::Custom;
        let remote_label = self
            .current_remote()
            .map(|remote| format!("测试将连接当前远端：{remote}"))
            .unwrap_or_else(|| "测试代理需要先打开带远端的仓库".to_string());

        let mut groups = vec![SettingGroup::new()
            .item(crate::settings_center::settings_group_heading(
                "代理模式",
                Some(proxy_mode_help(self.proxy_mode).into()),
            ))
            .item(SettingItem::new(
                "模式",
                SettingField::render(move |_options, _window, cx| {
                    view.update(cx, |this, cx| {
                        crate::ui::components::settings_segmented_group()
                            .child(this.proxy_mode_button("不使用代理", NetworkProxyMode::Disabled, cx))
                            .child(this.proxy_mode_button("使用系统代理", NetworkProxyMode::System, cx))
                            .child(this.proxy_mode_button("自定义代理", NetworkProxyMode::Custom, cx))
                    })
                }),
            ))];

        if custom_enabled {
            groups.push(
                SettingGroup::new()
                    .item(crate::settings_center::settings_group_heading(
                        "自定义地址",
                        Some("代理认证第一版请写在 URL 中，例如 http://user:pass@127.0.0.1:7890。".into()),
                    ))
                    .item({
                        let view = cx.entity();
                        SettingItem::new(
                            "HTTP 代理",
                            SettingField::render(move |_options, window, cx| {
                                view.update(cx, |this, cx| {
                                    this.input(FieldId::ProxyHttpUrl, false, window, cx)
                                })
                            }),
                        )
                    })
                    .item({
                        let view = cx.entity();
                        SettingItem::new(
                            "HTTPS 代理",
                            SettingField::render(move |_options, window, cx| {
                                view.update(cx, |this, cx| {
                                    this.input(FieldId::ProxyHttpsUrl, false, window, cx)
                                })
                            }),
                        )
                    })
                    .item({
                        let view = cx.entity();
                        SettingItem::new(
                            "SOCKS5 代理",
                            SettingField::render(move |_options, window, cx| {
                                view.update(cx, |this, cx| {
                                    this.input(FieldId::ProxySocks5Url, false, window, cx)
                                })
                            }),
                        )
                    }),
            );
        }

        groups.push(
            SettingGroup::new()
                .item(crate::settings_center::settings_group_heading(
                    "连接测试",
                    Some("在保存前可以先测试一次代理连通性。".into()),
                ))
                .item({
                    let view = cx.entity();
                    SettingItem::new(
                        "当前远端",
                        SettingField::render(move |_options, _window, cx| {
                            view.update(cx, |_this, _cx| {
                                div()
                                    .text_size(px(ui_theme::TYPE_BODY))
                                    .line_height(px(18.0))
                                    .text_color(rgb(ui_theme::CONTENT_SECONDARY))
                                    .child(remote_label.clone())
                            })
                        }),
                    )
                })
                .item({
                    let view = cx.entity();
                    SettingItem::render(move |_options, _window, cx| {
                        view.update(cx, |this, cx| {
                            dialog_actions()
                                .child(this.button(
                                    "测试代理",
                                    !this.busy,
                                    |this, _, _| this.test_network_proxy_settings(),
                                    cx,
                                ))
                                .child(this.primary_button(
                                    "保存",
                                    !this.busy,
                                    |this, _, cx| {
                                        this.save_network_proxy_settings();
                                        this.notify_settings_save("代理设置已保存", cx);
                                    },
                                    cx,
                                ))
                        })
                    }).keywords(["测试代理", "保存"])
                }),
        );

        groups
    }

    fn proxy_mode_button(
        &self,
        label: &'static str,
        mode: NetworkProxyMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.proxy_mode == mode;
        let enabled = !self.busy;
        let disabled_reason = proxy_mode_disabled_reason(enabled);
        segmented_button(format!("proxy-mode-{label}"), selected, enabled)
            .when_some(disabled_reason, |this, reason| {
                this.tooltip(move |_window, cx| tooltip_text(reason, cx))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if enabled && !this.busy {
                    this.set_proxy_mode(mode);
                    cx.notify();
                }
            }))
            .child(label)
    }
}

fn proxy_mode_disabled_reason(enabled: bool) -> Option<&'static str> {
    (!enabled).then_some("当前操作进行中，请稍候")
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

#[cfg(test)]
#[path = "tests/proxy_view.rs"]
mod tests;
