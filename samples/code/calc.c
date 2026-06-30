/* Sample C program — syntax highlighting demo. */
#include <stdio.h>
#include <stdlib.h>

#define MAX 16

/* Recursive factorial. */
static long factorial(int n) {
    if (n <= 1) {
        return 1L;
    }
    return (long)n * factorial(n - 1);
}

int main(void) {
    for (int i = 0; i < 5; i++) {
        printf("%d! = %ld\n", i, factorial(i));
    }
    const char *msg = "done";
    printf("%s (MAX=%d)\n", msg, MAX);
    return EXIT_SUCCESS;
}
