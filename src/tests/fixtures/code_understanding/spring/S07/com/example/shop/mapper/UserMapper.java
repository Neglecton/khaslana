package com.example.shop.mapper;

public interface UserMapper {
    String selectById(int id);

    String selectByName(String name);

    String selectByName(String name, int limit);
}
