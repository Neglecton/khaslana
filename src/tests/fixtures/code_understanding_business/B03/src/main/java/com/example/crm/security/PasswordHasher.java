package com.example.crm.security;

import org.springframework.stereotype.Component;

@Component
public class PasswordHasher {
    private static final String PREFIX = "bcrypt:";

    public boolean matches(String rawPassword, String storedHash) {
        return storedHash != null && storedHash.equals(PREFIX + rawPassword);
    }
}
