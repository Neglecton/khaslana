package com.example.shop.mapper;

public interface InventoryMapper {

    int lockedQty(String orderId);

    int release(String orderId, int qty);
}
