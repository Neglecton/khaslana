package com.example.web;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {

    private static final String DETAIL_PATH = "/detail";

    @GetMapping({"/{id}", DETAIL_PATH + "/{id}"})
    public String detail(String id) {
        return "order-" + id;
    }

    @PostMapping(path = "/batch")
    public String batch() {
        return "batch";
    }

    @RequestMapping("${legacy.base}/old")
    public String legacy() {
        return "legacy";
    }
}
