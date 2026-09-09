package com.example.job;

import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.amqp.rabbit.annotation.RabbitListener;
import org.springframework.context.event.EventListener;
import org.springframework.scheduling.annotation.Async;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

@Service
public class JobService {

    @Transactional
    public void settle() { }

    @Async
    public void warmCache() { }

    @Scheduled(cron = "0 0 3 * * ?")
    public void nightly() { }

    @KafkaListener(topics = "order-events")
    public void onOrderEvent(String payload) { }

    @EventListener
    public void onInternalEvent(Object event) { }

    @RabbitListener(queues = "order-queue")
    public void onOrderMessage(String payload) { }
}
