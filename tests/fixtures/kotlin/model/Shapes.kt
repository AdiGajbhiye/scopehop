package com.example.model

interface Named {
    val label: String
}

open class Shape(val name: String) : Named {
    override val label: String get() = name
    open fun area(): Double = 0.0
}

class Circle(val radius: Double) : Shape("circle") {
    override fun area(): Double = 3.14 * radius * radius

    companion object {
        fun unit(): Circle = Circle(1.0)
    }
}

class Square(val side: Double) : Shape("square") {
    override fun area(): Double = side * side
}

enum class Color(val hex: String) {
    RED("#f00"),
    GREEN("#0f0");

    fun shout(): String = hex.uppercase()
}

typealias ShapeList = List<Shape>

class Outer {
    class Inner {
        fun ping(): Int = 1
    }
}

sealed interface Mode {
    data object List : Mode
    data object Grid : Mode
}
