package com.example.crm.security;

import java.util.UUID;
import org.springframework.stereotype.Component;

@Component
public class TokenIssuer {
    public String issue(Long userId) {
        return "token-" + userId + "-" + UUID.randomUUID();
    }
}
