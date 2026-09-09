package com.example.web;

import com.example.svc.CacheStore;
import jakarta.annotation.Resource;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class CacheController {

    @Resource(name = "redisCache")
    private CacheStore store;

    @GetMapping("/cache/get")
    public String get(String key) {
        return store.get(key);
    }
}
