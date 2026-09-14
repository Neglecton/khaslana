package com.example.spring;

import org.apache.ibatis.annotations.Select;

public interface UserMapper {
    @Select("select id, username from users where username = #{username}")
    UserEntity findByUsername(String username);
}
