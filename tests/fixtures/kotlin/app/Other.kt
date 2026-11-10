package com.example.app

import com.example.model.Circle

// Same package as Main.kt, so visible there without an import.
fun helper(): Int = 1

fun makeCircle(radius: Double, label: String = ""): Circle = Circle(radius)
