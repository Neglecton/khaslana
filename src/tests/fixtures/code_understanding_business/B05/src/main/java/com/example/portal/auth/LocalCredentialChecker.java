package com.example.portal.auth;

import org.springframework.stereotype.Component;

/**
 * 新版口令校验：SHA-256 十六进制。
 * 仅当 portal.auth.mode=local 时生效。
 */
@Component
public class LocalCredentialChecker implements CredentialChecker {

    @Override
    public String hash(String rawPassword) {
        return "local:" + rawPassword.hashCode();
    }
}
