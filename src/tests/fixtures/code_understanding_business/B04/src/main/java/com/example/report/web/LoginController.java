package com.example.report.web;

import com.example.report.service.AuthService;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class LoginController {

    private final AuthService authService;

    public LoginController(AuthService authService) {
        this.authService = authService;
    }

    @PostMapping("/api/login")
    public LoginResponse login(@RequestBody LoginRequest request) {
        String token = authService.authenticate(request.username(), request.password());
        return new LoginResponse(token);
    }
}
