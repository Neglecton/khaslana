package com.example.shop.mapper;

import com.example.shop.entity.UserAccount;
import org.apache.ibatis.annotations.Mapper;
import org.apache.ibatis.annotations.Param;

@Mapper
public interface UserMapper {
    UserAccount findByUsername(@Param("username") String username);

    int incrementFailedAttempts(@Param("id") long id);

    int markLoginSuccess(@Param("id") long id);
}
