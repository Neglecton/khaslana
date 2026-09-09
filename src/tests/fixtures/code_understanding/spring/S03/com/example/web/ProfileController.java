package com.example.web;

import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class ProfileController {

    private final ProfileService singleCtor;
    @Autowired
    private ProfileService fieldInjected;
    private ProfileService setterInjected;

    public ProfileController(ProfileService singleCtor) {
        this.singleCtor = singleCtor;
    }

    @Autowired
    public void setSetterInjected(ProfileService setterInjected) {
        this.setterInjected = setterInjected;
    }

    @GetMapping("/profile/view")
    public String view() {
        return singleCtor.describe() + setterInjected.describe();
    }
}
