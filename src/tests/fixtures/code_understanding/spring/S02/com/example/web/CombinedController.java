package com.example.web;

import org.springframework.web.bind.annotation.RestController;

@RestController
public class CombinedController implements CombinedContract {

    @ApiGet("/combined/ping")
    public String ping() {
        return "pong";
    }

    @Override
    public String inherited() {
        return "inherited";
    }
}
