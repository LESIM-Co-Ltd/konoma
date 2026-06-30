// Sample C++ — syntax highlighting demo.
#include <iostream>
#include <string>
#include <vector>

template <typename T>
T sum(const std::vector<T> &xs) {
    T acc{};
    for (const auto &x : xs) {
        acc += x;
    }
    return acc;
}

class Greeter {
public:
    explicit Greeter(std::string name) : name_(std::move(name)) {}
    std::string hello() const { return "Hello, " + name_ + "!"; }

private:
    std::string name_;
};

int main() {
    std::vector<int> v{1, 2, 3, 4};
    Greeter g{"konoma"};
    std::cout << g.hello() << " sum=" << sum(v) << '\n';
    return 0;
}
