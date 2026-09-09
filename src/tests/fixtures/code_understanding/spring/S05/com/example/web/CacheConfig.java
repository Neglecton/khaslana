package com.example.web;

import com.example.svc.CacheStore;
import com.example.svc.RedisCacheStore;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;

@Configuration
public class CacheConfig {

    @Bean
    public CacheStore backupCache() {
        return new RedisCacheStore();
    }
}
