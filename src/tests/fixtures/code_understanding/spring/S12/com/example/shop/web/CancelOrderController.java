package com.example.shop.web;

import com.example.shop.service.OrderService;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class CancelOrderController {

    private final OrderService orderService;

    public CancelOrderController(OrderService orderService) {
        this.orderService = orderService;
    }

    @PostMapping("/{id}/cancel")
    public String cancel(@PathVariable String id) {
        return orderService.cancelOrder(id);
    }
}
