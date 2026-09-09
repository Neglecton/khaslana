package com.example.shop.service;

import com.example.shop.mapper.InventoryMapper;
import org.springframework.stereotype.Service;

@Service
public class DefaultOrderService implements OrderService {

    private final InventoryMapper inventoryMapper;

    public DefaultOrderService(InventoryMapper inventoryMapper) {
        this.inventoryMapper = inventoryMapper;
    }

    @Override
    public String cancelOrder(String orderId) {
        int qty = inventoryMapper.lockedQty(orderId);
        inventoryMapper.release(orderId, qty);
        return "cancelled";
    }
}
