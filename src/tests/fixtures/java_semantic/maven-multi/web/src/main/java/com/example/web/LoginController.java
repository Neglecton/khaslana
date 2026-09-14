package com.example.web;

import com.example.api.LoginResult;
import com.example.core.LoginApplicationService;

public final class LoginController {
    private final LoginApplicationService service;

    public LoginController(LoginApplicationService service) {
        this.service = service;
    }

    public LoginResult login(String username) {
        return service.login(username); // JLS: cross-module definition
    }
}
