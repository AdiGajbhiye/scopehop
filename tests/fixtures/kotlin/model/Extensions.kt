package com.example.model

// Bare `radius` is a member of the receiver.
fun Circle.diameter(): Double = radius * 2

// `List` is kotlin.collections.List, not Mode.List: a receiver brings its
// members into scope, not nested types.
fun List<Shape>.totalArea(): Double = sumOf { it.area() }

// `$name` in a string template is a member of the receiver too.
fun Shape.describe(): String = "$name has area ${area()}"
