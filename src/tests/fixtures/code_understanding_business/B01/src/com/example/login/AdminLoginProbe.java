package com.example.login;

public final class AdminLoginProbe {
    private final AuthService authService;

    public AdminLoginProbe(AuthService authService) {
        this.authService = authService;
    }

    public boolean verify(String username, String password) {
        try {
            authService.authenticate(username, password);
            return true;
        } catch (LoginRejectedException error) {
            return false;
        }
    }
}
