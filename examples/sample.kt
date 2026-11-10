package com.example.app

import com.example.util.Helper
import com.example.model.*

class Foo(val name: String, private var count: Int) : Base(), Iface {
    private val cache = mutableMapOf<String, Int>()
    var total: Int = 0

    fun bar(x: Int, y: String): Int {
        val local = x + count
        var other = local * 2
        for (i in 0 until x) { other += i }
        listOf(1, 2).forEach { item -> println(item + local) }
        listOf(3).map { it * 2 }
        try { risky() } catch (e: Exception) { println(e) }
        when (val r = compute()) { is Int -> r }
        val (a, b) = Pair(1, 2)
        this.total = other + a + b
        return Helper.help(local, name)
    }

    companion object {
        const val MAX = 10
        fun create(): Foo = Foo("x", 1)
    }
}

object Single { fun go() {} }
interface Iface { fun run() }
enum class Color { RED, GREEN }
typealias Handler = (Int) -> Unit
fun topLevel(p: Foo) = p.bar(1, "s")
val topProp = 3
fun String.ext(): String = this
// comment Foo
val s = "Foo string $topProp ${topProp + 1}"
