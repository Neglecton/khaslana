package com.example.portal.web;

import com.example.portal.auth.AuthResult;
import com.example.portal.auth.SsoAuthPort;
import com.example.portal.service.AuthService;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class AuthController {

    private final AuthService authService;

    public AuthController(AuthService authService) {
        this.authService = authService;
    }

    @PostMapping("/api/auth")
    public AuthResponse authenticate(@RequestBody AuthRequest request) {
        AuthResult result = authService.authenticate(request.username(), request.password());
        return new AuthResponse(result.token(), result.fromSso());
    }
}
