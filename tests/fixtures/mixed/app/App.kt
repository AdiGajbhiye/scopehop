package com.example.app

import com.example.lib.Greeter

// Kotlin using a Java class: constructor, instance and static members.
fun main() {
    val g = Greeter("x")
    g.greet()
    Greeter.of("y").greet()
    println(Greeter.HELLO)
    val b: Badge = Badge(g.badged())
    b.label()
}
