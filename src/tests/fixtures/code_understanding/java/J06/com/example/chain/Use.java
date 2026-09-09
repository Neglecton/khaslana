package com.example.chain;

import java.util.List;
import java.util.stream.Collectors;

public class Use {
    public String chained() {
        return new Builder()
            .setName("order")
            .setCount(2)
            .build();
    }

    public List<String> upperNames(List<String> names) {
        return names.stream()
            .filter(n -> !n.isEmpty())
            .map(String::toUpperCase)
            .collect(Collectors.toList());
    }
}
