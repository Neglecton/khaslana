public class Plain {
    public String greet() {
        return helper().join();
    }

    private Helper helper() {
        return new Helper();
    }
}
