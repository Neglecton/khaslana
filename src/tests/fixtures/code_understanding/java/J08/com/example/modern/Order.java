package com.example.modern;

public class Order {
    public enum Status { NEW, PAID, CANCELLED }

    public record Item(String sku, int qty) { }

    public sealed interface Reply permits Ok, Err { }
    public record Ok(Item item) implements Reply { }
    public record Err(String reason) implements Reply { }

    public String describe(Status status) {
        return switch (status) {
            case NEW -> "created";
            case PAID -> "paid";
            case CANCELLED -> "closed";
        };
    }

    public String render() {
        String tpl = """
            order detail
            """;
        return tpl;
    }

    public String itemLabel(Reply reply) {
        if (reply instanceof Ok(Item item)) {
            return item.sku();
        }
        return "unknown";
    }
}
