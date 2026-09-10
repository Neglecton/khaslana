package com.example.shop.entity;

public record UserAccount(long id, String username, String passwordHash, int status) {}
