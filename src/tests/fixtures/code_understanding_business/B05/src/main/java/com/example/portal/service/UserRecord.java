package com.example.portal.service;

/**
 * 服务层内部使用的用户记录。
 * 物理表见 UserRepository 对应的 Mapper XML。
 */
public record UserRecord(long id, String username, String passwordHash) {}
