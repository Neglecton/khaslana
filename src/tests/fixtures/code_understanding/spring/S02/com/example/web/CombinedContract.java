package com.example.web;

import org.springframework.web.bind.annotation.GetMapping;

public interface CombinedContract {

    @GetMapping("/combined/inherited")
    String inherited();
}
