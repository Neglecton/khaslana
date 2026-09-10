package com.example.login;

import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;

public final class SessionService {
    private final Map<String, Long> sessions = new ConcurrentHashMap<>();

    public String create(long userId) {
        String sessionId = UUID.randomUUID().toString();
        sessions.put(sessionId, userId);
        return sessionId;
    }
}
