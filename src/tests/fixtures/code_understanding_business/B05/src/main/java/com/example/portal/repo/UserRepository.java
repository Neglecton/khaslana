package com.example.portal.repo;

import com.example.portal.service.UserRecord;
import org.apache.ibatis.annotations.Mapper;
import org.apache.ibatis.annotations.Param;

@Mapper
public interface UserRepository {

    UserRecord findActive(@Param("month") String month, @Param("username") String username);

    int insertLoginEvent(@Param("month") String month, @Param("username") String username, @Param("result") String result);

    int archiveInactiveUsers(@Param("beforeDate") String beforeDate);
}
