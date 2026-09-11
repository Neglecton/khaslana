package com.example.portal.notification;

import org.springframework.stereotype.Component;

/**
 * 消息队列实现：把登录事件发到 MQ。
 * 不是数据库表访问，数据访问清单中应归为消息类。
 */
@Component
public class MqLoginNotifier implements LoginNotifier {

    @Override
    public void notifyLoginSucceeded(String username) {
        publish("auth.login.succeeded", username);
    }

    @Override
    public void notifyFailedLogin(String username) {
        publish("auth.login.failed", username);
    }

    private void publish(String routingKey, String payload) {
        // 真实实现连接 MQ；样例只表达边界。
    }
}
