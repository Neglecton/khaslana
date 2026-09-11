package com.example.portal.auth;

public record AuthResult(String token, boolean fromSso) {}
