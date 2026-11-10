package com.example.util

fun shout(s: String): String = s.uppercase()

// Same name as com.example.app.helper, but app never imports it.
fun helper(): Int = 0

const val VERSION = "1.0"

class Limits {
    companion object {
        const val MAX = 10
    }
}
