package com.example.legacy.web;

import com.example.legacy.annotations.Service;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@Service
@RestController
public class LegacyController {

    @GetMapping("/legacy/ping")
    public String ping() {
        return "pong";
    }
}
