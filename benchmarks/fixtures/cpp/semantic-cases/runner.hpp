#pragma once

namespace app {

class Base {
public:
    virtual int fallback(int value);
};

template <typename T>
class Runner : public Base {
public:
    T run(T value);
};

int helper(int value);
int call_runner(Runner<int>& runner);

}
