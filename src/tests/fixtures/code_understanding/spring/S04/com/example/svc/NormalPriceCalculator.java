package com.example.svc;

import org.springframework.context.annotation.Primary;
import org.springframework.stereotype.Service;

@Service
@Primary
public class NormalPriceCalculator implements PriceCalculator {
    @Override
    public int price(int base) {
        return base;
    }
}
