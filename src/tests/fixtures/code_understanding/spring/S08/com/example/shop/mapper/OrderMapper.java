package com.example.shop.mapper;

import java.util.List;
import org.apache.ibatis.annotations.Select;

public interface OrderMapper {

    @Select("select * from t_order where user_id = #{userId}")
    List<String> listByUser(int userId);

    List<String> listActive(String status);
}
