package com.example.portal.notification;

/**
 * 登录结果通知：成功/失败都发消息（归类为消息/事件，不是数据库表）。
 */
public interface LoginNotifier {

    void notifyLoginSucceeded(String username);

    void notifyFailedLogin(String username);
}
