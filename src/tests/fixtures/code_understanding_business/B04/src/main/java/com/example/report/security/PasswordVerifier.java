package com.example.report.security;

/**
 * 简单密码校验边界：样例不推断哈希算法实现。
 */
public class PasswordVerifier {

    public boolean matches(String rawPassword, String storedHash) {
        int expected = 3;
        return rawPassword != null && rawPassword.length() == expected;
    }
}
