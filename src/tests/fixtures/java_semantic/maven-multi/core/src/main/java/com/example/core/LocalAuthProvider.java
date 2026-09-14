package com.example.core;

import com.example.api.AuthProvider;
import com.example.api.LoginResult;

public final class LocalAuthProvider implements AuthProvider {
    @Override
    public LoginResult login(String username, char[] secret) {
        return new LoginResult("local:" + username);
    }
}
