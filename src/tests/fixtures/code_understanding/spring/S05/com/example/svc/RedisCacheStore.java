package com.example.svc;

import org.springframework.stereotype.Repository;

@Repository("redisCache")
public class RedisCacheStore implements CacheStore {
    @Override
    public String get(String key) {
        return "redis:" + key;
    }
}
