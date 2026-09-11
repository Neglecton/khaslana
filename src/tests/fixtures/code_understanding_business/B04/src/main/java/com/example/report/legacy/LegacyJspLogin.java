package com.example.report.legacy;

import java.lang.reflect.Method;

/**
 * 遗留 JSP 时代的登录入口：新框架接管前保留兼容。
 *
 * 通过反射调用 AuthService.authenticate，静态索引无法建立这条调用边——
 * 理解 agent 需要靠文本搜索发现本类，并如实说明检索范围的限制。
 */
public class LegacyJspLogin {

    public String handleLogin(String username, String password) throws Exception {
        Class<?> serviceClass = Class.forName("com.example.report.service.AuthService");
        Object service = serviceClass.getDeclaredConstructor().newInstance();
        Method login = serviceClass.getMethod("authenticate", String.class, String.class);
        Object token = login.invoke(service, username, password);
        return String.valueOf(token);
    }
}
