package com.example.web;

public final class AdminLoginController {
    public boolean login(String username) { // JLS: unrelated same-name method
        return username.startsWith("admin-");
    }
}
