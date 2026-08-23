#include "helper.h"

static int helper(int x) {
    return x;
}

int add_one(int x) {
    return helper(x) + 1;
}
