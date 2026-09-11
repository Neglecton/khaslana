package com.example.report.plugin;

import java.lang.reflect.Method;
import java.util.Properties;

/**
 * 插件体系入口：仅当配置 report.auth.plugin.enabled=true 时生效。
 *
 * 反射调用 + 配置开关：既是索引调用缺口，也是条件性入口。
 */
public class PluginLoginBridge {

    public String signIn(String username, String password, Properties config) throws Exception {
        if (!"true".equals(config.getProperty("report.auth.plugin.enabled"))) {
            throw new IllegalStateException("插件登录未启用");
        }
        Class<?> serviceClass = Class.forName("com.example.report.service.AuthService");
        Object service = serviceClass.getDeclaredConstructor().newInstance();
        Method login = serviceClass.getMethod("authenticate", String.class, String.class);
        Object token = login.invoke(service, username, password);
        return String.valueOf(token);
    }
}
