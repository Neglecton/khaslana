package com.example.report.entity;

public record UserAccount(long id, String username, String passwordHash, int status) {}
