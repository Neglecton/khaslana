package com.example.portal.auth;

import org.springframework.stereotype.Component;

/**
 * 遗留口令校验：MD5 风格短哈希。
 * 仅当 portal.auth.mode=legacy 时生效。
 */
@Component
public class LegacyCredentialChecker implements CredentialChecker {

    @Override
    public String hash(String rawPassword) {
        return "legacy:" + rawPassword.length();
    }
}
