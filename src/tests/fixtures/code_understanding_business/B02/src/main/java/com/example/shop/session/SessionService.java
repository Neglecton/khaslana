package com.example.shop.session;

import java.time.Duration;
import java.util.UUID;
import org.springframework.data.redis.core.StringRedisTemplate;
import org.springframework.stereotype.Service;

@Service
public class SessionService {
    private static final String SESSION_KEY_PREFIX = "session:";
    private static final Duration SESSION_TTL = Duration.ofHours(2);

    private final StringRedisTemplate redis;

    public SessionService(StringRedisTemplate redis) {
        this.redis = redis;
    }

    public String createSession(long userId) {
        String sessionId = UUID.randomUUID().toString();
        redis.opsForValue().set(SESSION_KEY_PREFIX + sessionId, String.valueOf(userId), SESSION_TTL);
        return sessionId;
    }
}
