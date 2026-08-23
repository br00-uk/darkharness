using System;

namespace Sample {
    public interface IGreeter {
        string Greet();
    }

    public class Greeting {
        public string Hello() {
            return Helper();
        }
    }
}
