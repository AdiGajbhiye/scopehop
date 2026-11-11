package com.example.app

import com.example.model.*
import com.example.model.Outer.Inner
import com.example.util.shout as yell
import com.example.util.VERSION
import com.example.util.Limits.Companion.MAX

class App(private val start: Circle) {
    private var count = 0

    fun run(): Int {
        val c = Circle(2.0)
        val r = c.radius
        val d = c.diameter()
        val a = start.area()
        val n = c.name
        val l = c.label
        val made = makeCircle(radius = 3.0, label = "x")
        val unit = Circle.unit()
        val nested: Outer.Inner = Outer.Inner()
        val inner: Inner = Inner()
        nested.ping()
        inner.ping()
        Color.RED.shout()
        yell(VERSION)
        helper()
        this.count += 1
        listOf(1, 2).map { it + count }
        listOf(c).forEach { shape -> shape.radius }
        for (i in 0 until 3) { count += i }
        try { made.area() } catch (e: Exception) { e.message }
        when (val s = made) { is Circle -> s.radius }
        val (first, second) = Pair(r, d)
        val greeting = "radius is $r"
        val mode: Mode = Mode.List
        // A method of this class, called without a receiver.
        val picked = pick().area()
        return count + first.toInt() + second.toInt()
    }

    fun pick(): Circle = start

    fun shadow(): Int {
        val count = 10
        return count
    }
}

fun limit(): Int = MAX

fun total(shapes: ShapeList): Double = shapes.totalArea()

fun untyped(items: List<Any>) {
    // `h` has no type, so `h.shape` matches all three holders. Two of the
    // three say Circle, so `.radius` goes to Circle.radius.
    items.forEach { h -> h.shape.radius }
}

class Shout : Shape("shout") {
    override fun area(): Double = super.area() + 1
}
