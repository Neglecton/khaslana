package com.example.shop.mapper;

import org.apache.ibatis.annotations.Mapper;
import org.apache.ibatis.annotations.Param;

@Mapper
public interface LoginLogMapper {
    int insertLog(@Param("username") String username, @Param("reason") String reason);
}
