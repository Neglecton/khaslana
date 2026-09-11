package com.example.portal.repo;

import org.apache.ibatis.annotations.Mapper;
import org.apache.ibatis.annotations.Param;

/**
 * 运营侧统计查询：复杂 SQL（CTE + JOIN + 别名）。
 * 与登录主流程无关，用于验收“CTE/别名不当物理表、JOIN 读源要分别列”。
 */
@Mapper
public interface StatsMapper {

    int countActiveLogins(@Param("month") String month);
}
