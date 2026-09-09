package com.example.svc;

import org.springframework.stereotype.Service;

@Service("vip")
public class VipPriceCalculator implements PriceCalculator {
    @Override
    public int price(int base) {
        return base / 2;
    }
}
