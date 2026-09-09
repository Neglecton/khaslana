package com.example.shop.entity;

import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.Id;
import jakarta.persistence.Table;

@Entity
@Table(name = "t_order")
public class OrderEntity {

    @Id
    private Long id;

    @Column(name = "order_no")
    private String orderNo;

    private String status;
}
