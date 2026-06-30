// Sample C header — syntax highlighting demo.
#ifndef KONOMA_SAMPLE_H
#define KONOMA_SAMPLE_H

#include <stddef.h>

#define VEC2_ZERO ((Vec2){0.0, 0.0})

typedef struct {
    double x;
    double y;
} Vec2;

Vec2 vec2_add(Vec2 a, Vec2 b);
double vec2_len(const Vec2 *v);
size_t vec2_hash(const Vec2 *v);

#endif /* KONOMA_SAMPLE_H */
