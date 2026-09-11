package com.example.portal.auth;

/**
 * 口令校验器：两个实现（LocalCredentialChecker / LegacyCredentialChecker），
 * 运行时按配置 portal.auth.mode 选择注入哪个。
 */
public interface CredentialChecker {

    String hash(String rawPassword);
}
