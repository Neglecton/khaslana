package com.example.report.mapper;

import org.apache.ibatis.annotations.Mapper;
import org.apache.ibatis.annotations.Param;

@Mapper
public interface LoginLogMapper {

    int insertLog(@Param("username") String username, @Param("result") String result);
}
