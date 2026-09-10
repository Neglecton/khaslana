package com.example.login;

public final class LoginController {
    private final AuthService authService;

    public LoginController(AuthService authService) {
        this.authService = authService;
    }

    public String login(String username, String password) {
        return authService.authenticate(username, password);
    }
}
