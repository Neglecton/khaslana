package com.example.web;

import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class AmbiguousCtorController {

    private final ProfileService service;

    public AmbiguousCtorController(ProfileService service) {
        this.service = service;
    }

    @Autowired
    public AmbiguousCtorController(ProfileService service, int flag) {
        this.service = service;
    }
}
