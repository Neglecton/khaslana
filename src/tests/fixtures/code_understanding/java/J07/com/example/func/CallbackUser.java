package com.example.func;

public class CallbackUser {
    public void run() { }

    public void wire() {
        Runnable lambda = () -> run();
        Runnable reference = this::run;
        Runnable anonymous = new Runnable() {
            @Override
            public void run() {
                run();
            }
        };
        class LocalHelper {
            void help() {
                run();
            }
        }
        lambda.run();
        reference.run();
        anonymous.run();
        new LocalHelper().help();
    }
}
