package com.example.api;

public interface AuthProvider {
    LoginResult login(String username, char[] secret); // JLS: interface implementation candidates
}
