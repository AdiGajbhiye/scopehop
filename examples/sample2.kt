object Single {
    fun go() = 1
}
interface Iface { fun run() }
enum class Color(val hex: Int) { RED(1), GREEN(2) }
typealias Handler = (Int) -> Unit
fun <T> topLevel(p: Foo, t: T) = p.bar(1, "s")
val topProp: Int get() = 3
fun String.ext(): String = "$this $topProp ${topProp + 1}"
class Bar<K> : Base {
    constructor(a: Int) : super(a) { use(a) }
    init { val z = 1 }
    data class Inner(val q: Int)
    val lazyThing by lazy { 3 }
    fun m() { val f = fun(w: Int) = w; Single.go(); Color.RED; Foo::bar; super.m() }
}
