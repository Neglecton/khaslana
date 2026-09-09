package com.example.web;

import com.example.svc.PriceCalculator;
import java.util.List;
import org.springframework.beans.factory.annotation.Qualifier;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class PriceController {

    private final PriceCalculator vip;
    private final List<PriceCalculator> all;

    public PriceController(@Qualifier("vip") PriceCalculator vip, List<PriceCalculator> all) {
        this.vip = vip;
        this.all = all;
    }

    @GetMapping("/price/vip")
    public int vip(int base) {
        return vip.price(base);
    }

    @GetMapping("/price/all")
    public int total(int base) {
        int sum = 0;
        for (PriceCalculator calculator : all) {
            sum += calculator.price(base);
        }
        return sum;
    }
}
