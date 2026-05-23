#include "runner.hpp"

namespace app {

int Base::fallback(int value) {
    return value;
}

int helper(int value) {
    return value + 1;
}

template <typename T>
T Runner<T>::run(T value) {
    return helper(value);
}

int call_runner(Runner<int>& runner) {
    return runner.run(1);
}

}
