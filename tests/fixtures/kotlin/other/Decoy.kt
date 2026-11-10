package com.example.other

// Same names as com.example.model.Circle and com.example.app.makeCircle.
// Nothing imports this package, so nothing should resolve here.
class Circle(val r: Double)

fun makeCircle(radius: Double): Circle = Circle(radius)
