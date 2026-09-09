package com.example.shop.repo;

import com.example.shop.entity.OrderEntity;
import java.util.List;
import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;
import org.springframework.data.repository.query.Param;

public interface OrderRepository extends JpaRepository<OrderEntity, Long> {

    List<OrderEntity> findByStatus(String status);

    @Query("select o from OrderEntity o where o.orderNo = :orderNo")
    List<OrderEntity> findByOrderNo(@Param("orderNo") String orderNo);
}
