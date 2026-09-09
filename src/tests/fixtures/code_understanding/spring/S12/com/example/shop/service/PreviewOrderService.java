package com.example.shop.service;

import org.springframework.context.annotation.Profile;
import org.springframework.stereotype.Service;

@Service("previewOrderService")
@Profile("preview")
public class PreviewOrderService implements OrderService {

    @Override
    public String cancelOrder(String orderId) {
        return "preview-cancelled";
    }
}
