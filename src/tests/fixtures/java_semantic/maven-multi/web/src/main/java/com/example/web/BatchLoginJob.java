package com.example.web;

import com.example.core.LoginApplicationService;

public final class BatchLoginJob {
    private final LoginApplicationService service;

    public BatchLoginJob(LoginApplicationService service) {
        this.service = service;
    }

    public void run() {
        service.login("batch-user");
    }
}
